//! Layer III Huffman coding of one granule's quantized lines: the big-values pairs in up to three
//! regions (each with the cheapest table), then the count1 quadruples, then nothing (rzero).
#![allow(clippy::needless_range_loop)]

use super::Bits;
use super::analysis::GRANULE;
use super::tables::{LINBITS, QUAD_A, T1, T2, T3, T5, T6, T7, T8, T9, T10, T11, T12, T13, T15, T16, T24};

/// The largest quantized magnitude a table can code: 15 plus 13 linbits.
pub(super) const MAX_IX: i32 = 15 + (1 << 13) - 1;

/// A code table: codes `(code, length)` indexed by `x * xlen + y`.
struct Code {
    codes: &'static [(u32, u8)],
    xlen: usize,
}

/// The distinct code tables: 1–15 without linbits, then the codes of 16–23 and of 24–31.
const CODES: [Code; 15] = [
    Code { codes: &T1, xlen: 2 },
    Code { codes: &T2, xlen: 3 },
    Code { codes: &T3, xlen: 3 },
    Code { codes: &T5, xlen: 4 },
    Code { codes: &T6, xlen: 4 },
    Code { codes: &T7, xlen: 6 },
    Code { codes: &T8, xlen: 6 },
    Code { codes: &T9, xlen: 6 },
    Code { codes: &T10, xlen: 8 },
    Code { codes: &T11, xlen: 8 },
    Code { codes: &T12, xlen: 8 },
    Code { codes: &T13, xlen: 16 },
    Code { codes: &T15, xlen: 16 },
    Code { codes: &T16, xlen: 16 },
    Code { codes: &T24, xlen: 16 },
];

/// The table numbers of `CODES[..13]`.
const PLAIN_IDS: [u32; 13] = [1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15];

/// The bits of a pair a table cannot code.
const INVALID: u32 = 1 << 20;

/// The code table, linbits and whether escapes apply, of table number `id` (1–31, not 4 or 14).
fn table(id: u32) -> Option<(&'static Code, u32, bool)> {
    match id {
        16..=23 => Some((&CODES[13], LINBITS[(id - 16) as usize], true)),
        24..=31 => Some((&CODES[14], LINBITS[(id - 16) as usize], true)),
        _ => PLAIN_IDS.iter().position(|&t| t == id).map(|i| (&CODES[i], 0, false)),
    }
}

/// How a granule's lines are coded.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Coding {
    /// Pairs coded with the big-values tables.
    pub big_values: usize,
    /// End of the count1 region (a line index); lines from here on are zero.
    pub count1_end: usize,
    pub table_select: [u32; 3],
    pub region0_count: u32,
    pub region1_count: u32,
    /// Count1 table B (fixed four-bit codes) instead of A.
    pub count1_b: bool,
    /// Bits of the Huffman-coded data (part 3).
    pub bits: u32,
}

/// The bits of pair `(x, y)` in code table `c` (escapes counted separately), or [`INVALID`].
fn pair_bits(c: &Code, x: i32, y: i32, esc: bool) -> u32 {
    let (cx, cy) = if esc { (x.min(15), y.min(15)) } else { (x, y) };
    let (Ok(cx), Ok(cy)) = (usize::try_from(cx), usize::try_from(cy)) else { return INVALID };
    if cx >= c.xlen || cy >= c.xlen {
        return INVALID;
    }
    c.codes.get(cx * c.xlen + cy).map_or(INVALID, |&(_, len)| u32::from(len) + u32::from(x != 0) + u32::from(y != 0))
}

/// The count1 and rzero regions of `ix`: the coding with no big-values regions planned yet.
fn count1(ix: &[i32; GRANULE]) -> Coding {
    let mut end = GRANULE;
    while end >= 2 && ix[end - 1] == 0 && ix[end - 2] == 0 {
        end -= 2;
    }
    let (mut start, mut bits_a, mut bits_b) = (end, 0u32, 0u32);
    while start >= 4 {
        let q = &ix[start - 4..start];
        if q.iter().any(|&v| v > 1) {
            break;
        }
        let signs = q.iter().filter(|&&v| v != 0).count() as u32;
        let (_, len) = QUAD_A[quad_index(q)];
        bits_a += u32::from(len) + signs;
        bits_b += 4 + signs;
        start -= 4;
    }
    Coding { big_values: start / 2, count1_end: end, count1_b: bits_b < bits_a, bits: bits_a.min(bits_b), ..Coding::default() }
}

/// A quick coding of `ix` for the rate loops: regions split by a fixed rule, each coded with the
/// cheapest of the few tables sized for its largest value. Never cheaper than [`plan`].
pub(super) fn estimate(ix: &[i32; GRANULE], sfb: &[usize; 23]) -> Coding {
    let mut coding = count1(ix);
    let (pairs, start) = (coding.big_values, coding.big_values * 2);
    if pairs == 0 {
        return coding;
    }
    // About a third of the bands in each region.
    let bands = (1..=22).filter(|&i| sfb[i] <= start).count();
    let r0 = (bands / 3).saturating_sub(1).min(15);
    let r1 = (2 * bands / 3).saturating_sub(r0 + 2).min(7);
    let bounds = [0, sfb[r0 + 1].min(start) / 2, sfb[r0 + r1 + 2].min(start) / 2, pairs];
    for r in 0..3 {
        let (id, bits) = cheapest(ix, bounds[r], bounds[r + 1].max(bounds[r]));
        coding.table_select[r] = id;
        coding.bits = coding.bits.saturating_add(bits);
    }
    coding.region0_count = r0 as u32;
    coding.region1_count = r1 as u32;
    coding
}

/// The cheapest table (and its bits) for pairs `s..e` among those sized for their largest value.
fn cheapest(ix: &[i32; GRANULE], s: usize, e: usize) -> (u32, u32) {
    let pairs = ix.get(2 * s..2 * e).unwrap_or_default();
    let max = pairs.iter().copied().max().unwrap_or(0);
    let plain: &[usize] = match max {
        0 => return (0, 0),
        1 => &[0, 1, 2],
        2 => &[1, 2, 3, 4],
        3 => &[3, 4],
        4 | 5 => &[5, 6, 7],
        6 | 7 => &[8, 9, 10],
        8..=15 => &[11, 12],
        _ => &[],
    };
    let mut best = (0, u32::MAX);
    for &t in plain {
        let bits = pairs.as_chunks::<2>().0.iter().map(|&[x, y]| pair_bits(&CODES[t], x, y, false)).fold(0u32, u32::saturating_add);
        if bits < best.1 {
            best = (PLAIN_IDS[t], bits);
        }
    }
    if max >= 8 {
        let escaped = pairs.iter().filter(|&&v| v >= 15).count() as u32;
        for (t, first) in [(13usize, 16u32), (14, 24)] {
            let Some(k) = (0..8u32).find(|&k| max <= 14 + (1 << LINBITS[(first - 16 + k) as usize])) else { continue };
            let base = pairs.as_chunks::<2>().0.iter().map(|&[x, y]| pair_bits(&CODES[t], x, y, true)).fold(0u32, u32::saturating_add);
            let bits = base.saturating_add(LINBITS[(first - 16 + k) as usize] * escaped);
            if bits < best.1 {
                best = (first + k, bits);
            }
        }
    }
    best
}

/// The cheapest coding of the magnitudes `ix` (each at most [`MAX_IX`]) with the long-block
/// scalefactor bands `sfb`: every region split, and every table for each region.
pub(super) fn plan(ix: &[i32; GRANULE], sfb: &[usize; 23]) -> Coding {
    let mut coding = count1(ix);
    let (pairs, start) = (coding.big_values, coding.big_values * 2);
    if pairs == 0 {
        return coding;
    }

    // Prefix sums of each table's pair bits and of escaped values.
    let mut pre = [[0u32; GRANULE / 2 + 1]; 15];
    let mut esc = [0u32; GRANULE / 2 + 1];
    for p in 0..pairs {
        let (x, y) = (ix[2 * p], ix[2 * p + 1]);
        for (t, c) in CODES.iter().enumerate() {
            pre[t][p + 1] = pre[t][p].saturating_add(pair_bits(c, x, y, t >= 13));
        }
        esc[p + 1] = esc[p] + u32::from(x >= 15) + u32::from(y >= 15);
    }
    // Region boundaries fall on scalefactor bands; band maxima are clipped to the big values.
    let bound = |i: usize| sfb[i].min(start) / 2;
    let mut band_max = [0i32; 22];
    for (b, m) in band_max.iter_mut().enumerate() {
        *m = ix[sfb[b].min(start)..sfb[b + 1].min(start)].iter().copied().max().unwrap_or(0);
    }
    let best = |s: usize, e: usize, max: i32| -> (u32, u32) {
        if s >= e || max == 0 {
            return (0, 0);
        }
        let mut best = (0, u32::MAX);
        for (t, &id) in PLAIN_IDS.iter().enumerate() {
            if CODES[t].xlen as i32 > max {
                let bits = pre[t][e] - pre[t][s];
                if bits < best.1 {
                    best = (id, bits);
                }
            }
        }
        let escaped = esc[e] - esc[s];
        for (t, first) in [(13usize, 16u32), (14, 24)] {
            let Some(k) = (0..8u32).find(|&k| max <= 14 + (1 << LINBITS[(first - 16 + k) as usize])) else { continue };
            let linbits = LINBITS[(first - 16 + k) as usize];
            let bits = (pre[t][e] - pre[t][s]).saturating_add(linbits * escaped);
            if bits < best.1 {
                best = (first + k, bits);
            }
        }
        best
    };
    let max_of = |a: usize, b: usize| band_max[a..b].iter().copied().max().unwrap_or(0);

    let mut chosen = (u32::MAX, [0u32; 3], 0u32, 0u32);
    for i in 1..=16 {
        let r0 = best(0, bound(i), max_of(0, i));
        for j in i + 1..=(i + 8).min(22) {
            let r1 = best(bound(i), bound(j), max_of(i, j));
            let r2 = best(bound(j), pairs, max_of(j, 22));
            let bits = r0.1.saturating_add(r1.1).saturating_add(r2.1);
            if bits < chosen.0 {
                chosen = (bits, [r0.0, r1.0, r2.0], (i - 1) as u32, (j - i - 1) as u32);
            }
        }
    }
    coding.table_select = chosen.1;
    coding.region0_count = chosen.2;
    coding.region1_count = chosen.3;
    coding.bits = coding.bits.saturating_add(chosen.0);
    coding
}

fn quad_index(q: &[i32]) -> usize {
    q.iter().fold(0, |acc, &v| (acc << 1) | usize::from(v != 0))
}

/// Writes the Huffman-coded lines `ix` (magnitudes; `neg` marks negative lines) as planned.
pub(super) fn write(out: &mut Bits, ix: &[i32; GRANULE], neg: &[bool; GRANULE], coding: &Coding, sfb: &[usize; 23]) {
    let region1 = sfb[coding.region0_count as usize + 1];
    let region2 = sfb[(coding.region0_count + coding.region1_count) as usize + 2];
    for p in 0..coding.big_values {
        let i = 2 * p;
        let id = coding.table_select[if i < region1 {
            0
        } else if i < region2 {
            1
        } else {
            2
        }];
        let Some((c, linbits, esc)) = table(id) else { continue };
        let (x, y) = (ix[i], ix[i + 1]);
        let (cx, cy) = if esc { (x.min(15), y.min(15)) } else { (x, y) };
        let (code, len) = c.codes.get(cx as usize * c.xlen + cy as usize).copied().unwrap_or((0, 0));
        out.put(code, u32::from(len));
        for (v, n) in [(x, neg[i]), (y, neg[i + 1])] {
            if esc && v >= 15 {
                out.put((v - 15) as u32, linbits);
            }
            if v != 0 {
                out.put(u32::from(n), 1);
            }
        }
    }
    let mut i = coding.big_values * 2;
    while i + 4 <= coding.count1_end {
        let q = &ix[i..i + 4];
        let k = quad_index(q);
        if coding.count1_b {
            out.put(15 - k as u32, 4);
        } else {
            let (code, len) = QUAD_A[k];
            out.put(u32::from(code), u32::from(len));
        }
        for j in i..i + 4 {
            if ix[j] != 0 {
                out.put(u32::from(neg[j]), 1);
            }
        }
        i += 4;
    }
}
