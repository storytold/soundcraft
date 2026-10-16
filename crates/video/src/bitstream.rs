//! Bit-level reading and writing shared by FilmCraft's codec crates.
//!
//! - [`BitReader`]: MSB-first reader with Exp-Golomb (`ue(v)`, `se(v)`), as used by H.264/HEVC/AV1 headers.
//! - [`BitWriter`]: the matching writer (for encoders and header rewriting).
//! - [`unescape_rbsp`] / [`escape_rbsp`]: remove/insert H.264/HEVC emulation-prevention bytes.
//! - [`annexb_nals`] / [`length_prefixed_nals`]: split Annex-B byte streams and AVCC/HVCC samples into NAL units.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitError {
    /// Tried to read past the end of the buffer.
    Eof,
    /// An Exp-Golomb code longer than 32 leading zeros.
    InvalidExpGolomb,
    /// A NAL length-prefix width outside the AVCC/HVCC range of 1..=4 bytes.
    InvalidLengthSize,
}

impl fmt::Display for BitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BitError::Eof => f.write_str("unexpected end of bitstream"),
            BitError::InvalidExpGolomb => f.write_str("invalid Exp-Golomb code"),
            BitError::InvalidLengthSize => f.write_str("invalid NAL length size"),
        }
    }
}
impl std::error::Error for BitError {}

pub type Result<T> = std::result::Result<T, BitError>;

/// MSB-first bit reader over a byte slice.
#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Position in bits.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
    /// Bit position from the start.
    pub fn position(&self) -> usize {
        self.pos
    }
    pub fn bits_left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }
    pub fn is_byte_aligned(&self) -> bool {
        self.pos.is_multiple_of(8)
    }
    pub fn byte_align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }
    /// Byte offset of the current position (rounded down).
    pub fn byte_pos(&self) -> usize {
        self.pos / 8
    }
    pub fn seek_bits(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Peek up to 32 bits without consuming (zero-padded past the end).
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        let byte = self.pos / 8;
        let shift = self.pos % 8;
        let mut v: u64 = 0;
        for i in 0..5 {
            v = (v << 8) | *self.data.get(byte + i).unwrap_or(&0) as u64;
        }
        // v holds 40 bits starting at `byte`
        ((v << (24 + shift)) >> (64 - n)) as u32
    }

    #[inline]
    pub fn read_bits(&mut self, n: u32) -> Result<u32> {
        if n as usize > self.bits_left() {
            return Err(BitError::Eof);
        }
        let v = self.peek(n);
        self.pos += n as usize;
        Ok(v)
    }

    pub fn read_bits_u64(&mut self, n: u32) -> Result<u64> {
        if n <= 32 {
            return self.read_bits(n).map(u64::from);
        }
        let hi = self.read_bits(n - 32)? as u64;
        let lo = self.read_bits(32)? as u64;
        Ok((hi << 32) | lo)
    }

    #[inline]
    pub fn read_bit(&mut self) -> Result<bool> {
        self.read_bits(1).map(|b| b == 1)
    }
    pub fn read_flag(&mut self) -> Result<bool> {
        self.read_bit()
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        if n > self.bits_left() {
            return Err(BitError::Eof);
        }
        self.pos += n;
        Ok(())
    }

    /// Unsigned Exp-Golomb `ue(v)`.
    pub fn read_ue(&mut self) -> Result<u32> {
        let mut zeros = 0u32;
        while !self.read_bit()? {
            zeros += 1;
            if zeros > 32 {
                return Err(BitError::InvalidExpGolomb);
            }
        }
        if zeros == 0 {
            return Ok(0);
        }
        let v = self.read_bits_u64(zeros)?;
        let r = (1u64 << zeros) - 1 + v;
        u32::try_from(r).map_err(|_| BitError::InvalidExpGolomb)
    }

    /// Signed Exp-Golomb `se(v)`.
    pub fn read_se(&mut self) -> Result<i32> {
        let k = self.read_ue()? as i64;
        Ok(if k & 1 == 1 { ((k + 1) / 2) as i32 } else { -((k / 2) as i32) })
    }

    /// Truncated Exp-Golomb `te(v)` with range `max`.
    pub fn read_te(&mut self, max: u32) -> Result<u32> {
        if max > 1 { self.read_ue() } else { Ok(!self.read_bit()? as u32) }
    }

    /// H.264/HEVC `more_rbsp_data()`: true if there is data before the rbsp trailing bits.
    pub fn more_rbsp_data(&self) -> bool {
        let total = self.data.len() * 8;
        if self.pos >= total {
            return false;
        }
        // find last set bit (the rbsp_stop_one_bit)
        let mut last = None;
        for (i, &b) in self.data.iter().enumerate().rev() {
            if b != 0 {
                last = Some(i * 8 + 7 - b.trailing_zeros() as usize);
                break;
            }
        }
        match last {
            Some(stop) => self.pos < stop,
            None => false,
        }
    }
}

/// MSB-first bit writer.
#[derive(Default, Clone)]
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn write_bits(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        let v = (value as u64) & ((1u64 << n) - 1);
        self.acc = (self.acc << n) | v;
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.buf.push((self.acc >> self.nbits) as u8);
        }
        self.acc &= (1u64 << self.nbits) - 1;
    }
    pub fn write_bit(&mut self, b: bool) {
        self.write_bits(b as u32, 1);
    }
    pub fn write_ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let len = 64 - x.leading_zeros();
        let zeros = len - 1;
        for _ in 0..zeros {
            self.write_bit(false);
        }
        if len > 32 {
            self.write_bits((x >> 32) as u32, len - 32);
            self.write_bits(x as u32, 32);
        } else {
            self.write_bits(x as u32, len);
        }
    }
    pub fn write_se(&mut self, v: i32) {
        let k = if v > 0 { (v as u32) * 2 - 1 } else { (-(v as i64) as u32) * 2 };
        self.write_ue(k);
    }
    pub fn bit_len(&self) -> usize {
        self.buf.len() * 8 + self.nbits as usize
    }
    pub fn is_byte_aligned(&self) -> bool {
        self.nbits == 0
    }
    /// Pad with zero bits to a byte boundary.
    pub fn align_zero(&mut self) {
        if self.nbits > 0 {
            self.write_bits(0, 8 - self.nbits);
        }
    }
    /// `rbsp_trailing_bits()`: a one bit then zeros to alignment.
    pub fn rbsp_trailing(&mut self) {
        self.write_bit(true);
        self.align_zero();
    }
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        if self.nbits == 0 {
            self.buf.extend_from_slice(bytes);
        } else {
            for &b in bytes {
                self.write_bits(b as u32, 8);
            }
        }
    }
    pub fn finish(mut self) -> Vec<u8> {
        self.align_zero();
        self.buf
    }
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }
}

/// Remove emulation-prevention bytes (`00 00 03` → `00 00`) from a NAL payload.
pub fn unescape_rbsp(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Insert emulation-prevention bytes so no `00 00 0x` (x ≤ 3) sequence appears.
pub fn escape_rbsp(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 64);
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Split an Annex-B byte stream (start codes `00 00 01` / `00 00 00 01`) into NAL units (escaped payloads).
pub fn annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut e = if k + 1 < starts.len() { starts[k + 1] - 3 } else { data.len() };
        // trim trailing zero bytes (part of a 4-byte start code or trailing_zero_8bits)
        while e > s && data[e - 1] == 0 {
            e -= 1;
        }
        if e > s {
            out.push(&data[s..e]);
        }
    }
    out
}

/// Split a length-prefixed sample (AVCC/HVCC, `length_size` 1..=4 bytes) into NAL units.
///
/// The width comes from the stream's configuration record (`(byte & 3) + 1`), so anything
/// outside 1..=4 is a programming or parsing error and is rejected up front: a width of 0
/// would never advance the cursor, looping forever and appending empty NALs until the
/// process runs out of memory, and a wider prefix cannot occur in a real sample.
pub fn length_prefixed_nals(data: &[u8], length_size: usize) -> Result<Vec<&[u8]>> {
    if !(1..=4).contains(&length_size) {
        return Err(BitError::InvalidLengthSize);
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let Some(start) = i.checked_add(length_size) else {
            return Err(BitError::Eof);
        };
        if start > data.len() {
            return Err(BitError::Eof);
        }
        let mut len = 0usize;
        for &b in &data[i..start] {
            len = (len << 8) | b as usize;
        }
        let Some(end) = start.checked_add(len) else {
            return Err(BitError::Eof);
        };
        if end > data.len() {
            return Err(BitError::Eof);
        }
        out.push(&data[start..end]);
        i = end;
    }
    Ok(out)
}

/// CRC-32 (IEEE, reflected) — used by containers (e.g. Matroska CRC-32 elements, PNG).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn reads_bits() {
        let d = [0b1010_1100, 0b0101_0011];
        let mut r = BitReader::new(&d);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(3).unwrap(), 0b010);
        assert_eq!(r.read_bits(8).unwrap(), 0b1100_0101);
        assert_eq!(r.bits_left(), 4);
        assert_eq!(r.read_bits(5), Err(BitError::Eof));
    }

    #[test]
    fn exp_golomb_known() {
        // 1 | 010 | 011 | 00100 | 00101 -> 0,1,2,3,4
        let mut w = BitWriter::new();
        for v in 0..5 {
            w.write_ue(v);
        }
        let b = w.finish();
        let mut r = BitReader::new(&b);
        for v in 0..5 {
            assert_eq!(r.read_ue().unwrap(), v);
        }
        let mut w = BitWriter::new();
        for v in [0, 1, -1, 2, -2, 1000, -1000] {
            w.write_se(v);
        }
        let b = w.finish();
        let mut r = BitReader::new(&b);
        for v in [0, 1, -1, 2, -2, 1000, -1000] {
            assert_eq!(r.read_se().unwrap(), v);
        }
    }

    #[test]
    fn emulation_prevention() {
        let raw = [0, 0, 1, 0, 0, 0, 5, 0, 0, 3];
        let esc = escape_rbsp(&raw);
        assert_eq!(esc, vec![0, 0, 3, 1, 0, 0, 3, 0, 5, 0, 0, 3, 3]);
        assert_eq!(unescape_rbsp(&esc), raw);
    }

    #[test]
    fn annexb_split() {
        let s = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        let n = annexb_nals(&s);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]);
        let l = [0, 0, 0, 2, 0x67, 1, 0, 0, 0, 1, 0x68];
        assert_eq!(length_prefixed_nals(&l, 4).unwrap(), vec![&[0x67, 1][..], &[0x68][..]]);
    }

    #[test]
    fn length_prefixed_rejects_bad_length_size() {
        // Regression test for #469: with a width of 0 neither the prefix nor a
        // zero-length payload advanced the cursor, so any non-empty sample
        // looped forever appending empty NALs. Widths above 4 cannot come
        // from an AVCC/HVCC record either.
        let d = [0, 0, 0, 2, 0x67, 1];
        assert_eq!(length_prefixed_nals(&d, 0), Err(BitError::InvalidLengthSize));
        assert_eq!(length_prefixed_nals(&d, 5), Err(BitError::InvalidLengthSize));
        assert_eq!(length_prefixed_nals(&d, usize::MAX), Err(BitError::InvalidLengthSize));
        assert_eq!(length_prefixed_nals(&[], 0), Err(BitError::InvalidLengthSize));
    }

    #[test]
    fn length_prefixed_widths() {
        // 1-byte prefixes.
        assert_eq!(length_prefixed_nals(&[2, 0x67, 1, 1, 0x68], 1).unwrap(), vec![&[0x67, 1][..], &[0x68][..]]);
        // 2-byte prefixes.
        assert_eq!(length_prefixed_nals(&[0, 2, 0x67, 1, 0, 1, 0x68], 2).unwrap(), vec![&[0x67, 1][..], &[0x68][..]]);
        // 3-byte prefixes (lengthSizeMinusOne == 2 in the configuration record).
        assert_eq!(length_prefixed_nals(&[0, 0, 2, 0x67, 1], 3).unwrap(), vec![&[0x67, 1][..]]);
        // A truncated prefix or payload is Eof, not a panic or a short read.
        assert_eq!(length_prefixed_nals(&[0, 0], 4), Err(BitError::Eof));
        assert_eq!(length_prefixed_nals(&[0, 0, 0, 9, 0x67], 4), Err(BitError::Eof));
        // Empty input splits to no NALs.
        assert!(length_prefixed_nals(&[], 4).unwrap().is_empty());
    }

    #[test]
    fn more_rbsp() {
        let d = [0b1100_0000];
        let mut r = BitReader::new(&d);
        assert!(r.more_rbsp_data());
        r.read_bit().unwrap();
        assert!(!r.more_rbsp_data());
    }

    #[test]
    fn crc() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    proptest! {
        #[test]
        fn roundtrip(vals in proptest::collection::vec((0u32..u32::MAX, 1u32..=32), 0..200), ues in proptest::collection::vec(0u32..u32::MAX - 1, 0..50)) {
            let mut w = BitWriter::new();
            for &(v, n) in &vals { w.write_bits(v, n); }
            for &u in &ues { w.write_ue(u); }
            let b = w.finish();
            let mut r = BitReader::new(&b);
            for &(v, n) in &vals {
                let mask = if n == 32 { u32::MAX } else { (1 << n) - 1 };
                prop_assert_eq!(r.read_bits(n).unwrap(), v & mask);
            }
            for &u in &ues { prop_assert_eq!(r.read_ue().unwrap(), u); }
        }

        #[test]
        fn escape_roundtrip(data in proptest::collection::vec(0u8..4, 0..300)) {
            let e = escape_rbsp(&data);
            prop_assert_eq!(unescape_rbsp(&e), data);
            for w in e.windows(3) { prop_assert!(!(w[0] == 0 && w[1] == 0 && w[2] < 3)); }
        }
    }
}
