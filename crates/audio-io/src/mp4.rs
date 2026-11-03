//! Size checks on MP4 atoms before symphonia reads them: it allocates whatever an atom, a sample
//! table or a sample claims, and a failed allocation aborts the process.

use crate::pcm::{be_u32, be_u64};
use crate::{AudioError, Result};

/// The most `stts` entries a sound track may have. Symphonia finds each packet's timestamp by
/// scanning the table from its start, so a table of one-sample runs makes decoding quadratic in
/// the packet count. Audio encoders write one run per packet length (a handful of entries);
/// this bound keeps the worst case at a few hundred million steps.
const MAX_AUDIO_STTS_ENTRIES: u32 = 1 << 14;

#[derive(Clone, Copy)]
enum Level {
    Top,
    Moov,
    Trak,
    Mdia,
    /// `audio`: the `mdia` has a `soun` handler, so symphonia will decode its packets.
    Minf {
        audio: bool,
    },
    Stbl {
        audio: bool,
    },
    Udta,
    Meta,
    Ilst,
    Tag,
}

/// Refuse the MP4 file whose top-level atoms start at `at` if an atom symphonia reads whole, a
/// sample table or a sample claims more bytes than the file holds.
pub(crate) fn check(b: &[u8], at: usize) -> Result<()> {
    atoms(b, at, b.len(), Level::Top).map(|_| ())
}

fn malformed(what: &str) -> AudioError {
    AudioError::Malformed(format!("MP4 {what} is larger than the file"))
}

/// Walk the atoms in `pos..end` that symphonia descends into, as it reads them. False where
/// symphonia stops reading: at a header it refuses, or after an atom that runs past `end`.
fn atoms(b: &[u8], mut pos: usize, end: usize, level: Level) -> Result<bool> {
    let start = pos;
    // A header that starts inside the container is read whole, even past its end.
    while pos < end {
        let (Some(size), Some(kind)) = (be_u32(b, pos), b.get(pos + 4..pos + 8)) else { return Ok(false) };
        let head = if size == 1 { 16 } else { 8 };
        // Size 0: no data, and the next atom starts at the container's end.
        let len = match size {
            0 => head,
            1 => be_u64(b, pos + 8).unwrap_or(0),
            n => u64::from(n),
        };
        if len < head {
            return Ok(false);
        }
        let body = pos + head as usize;
        let data_end = usize::try_from(len).ok().and_then(|n| pos.checked_add(n));
        let next = if size == 0 { Some(end) } else { data_end };
        let inner = data_end.map_or(b.len(), |n| n.min(b.len()));
        let more = match (level, kind) {
            (Level::Top, b"moov") => atoms(b, body, inner, Level::Moov)?,
            (Level::Top | Level::Udta, b"meta") => atoms(b, body + 4, inner, Level::Meta)?,
            (Level::Moov, b"trak") => atoms(b, body, inner, Level::Trak)?,
            (Level::Moov, b"udta") => atoms(b, body, inner, Level::Udta)?,
            (Level::Trak, b"mdia") => atoms(b, body, inner, Level::Mdia)?,
            (Level::Mdia, b"minf") => atoms(b, body, inner, Level::Minf { audio: handler_is_sound(b, start, end) })?,
            (Level::Minf { audio }, b"stbl") => atoms(b, body, inner, Level::Stbl { audio })?,
            (Level::Meta, b"ilst") => atoms(b, body, inner, Level::Ilst)?,
            (Level::Ilst, _) => atoms(b, body, inner, Level::Tag)?,
            (Level::Stbl { audio }, b"stts") => {
                let fits = table(b, body + 4, 8, inner)?;
                if audio && be_u32(b, body + 4).is_some_and(|n| n > MAX_AUDIO_STTS_ENTRIES) {
                    return Err(AudioError::Malformed("MP4 audio timing table has more runs than packets could need".into()));
                }
                fits
            }
            (Level::Stbl { .. }, b"co64") => table(b, body + 4, 8, inner)?,
            (Level::Stbl { .. }, b"stsc") => table(b, body + 4, 12, inner)?,
            (Level::Stbl { .. }, b"stco") => table(b, body + 4, 4, inner)?,
            (Level::Stbl { .. }, b"stsz") => sample_sizes(b, body, inner)?,
            (Level::Mdia, b"hdlr") | (Level::Tag, b"data" | b"name" | b"mean") if len > (b.len() - pos) as u64 => {
                return Err(malformed(&format!("`{}` atom", String::from_utf8_lossy(kind))));
            }
            _ => true,
        };
        match next {
            Some(n) if more && n <= end => pos = n,
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Whether the `mdia` atom whose children lie in `pos..end` declares a `soun` handler: its
/// `hdlr` is usually before `minf`, but may follow it, so the children are walked once more.
fn handler_is_sound(b: &[u8], mut pos: usize, end: usize) -> bool {
    while pos < end {
        let (Some(size), Some(kind)) = (be_u32(b, pos), b.get(pos + 4..pos + 8)) else { return false };
        let head = if size == 1 { 16 } else { 8 };
        if kind == b"hdlr" {
            // Version and flags, then the predefined field, then the handler type.
            return b.get(pos + head + 8..pos + head + 12) == Some(b"soun");
        }
        let len = match size {
            0 => return false,
            1 => be_u64(b, pos + 8).unwrap_or(0),
            n => u64::from(n),
        };
        match usize::try_from(len).ok().filter(|&n| n >= head).and_then(|n| pos.checked_add(n)) {
            Some(next) => pos = next,
            None => return false,
        }
    }
    false
}

/// A sample table's entries, counted at `count`, must fit in the file: symphonia reserves room for
/// all of them first. False if they run past the table's atom (`end`), where symphonia stops.
fn table(b: &[u8], count: usize, entry: u64, end: usize) -> Result<bool> {
    let Some(n) = be_u32(b, count) else { return Ok(false) };
    let (first, bytes) = (count + 4, u64::from(n) * entry);
    if bytes > b.len().saturating_sub(first) as u64 {
        return Err(malformed("sample table"));
    }
    Ok(first as u64 + bytes <= end as u64)
}

/// Every sample's size must fit in the file: symphonia reads each one into a buffer of that size.
/// Sizes past the atom are never used, as symphonia stops before reading a packet.
fn sample_sizes(b: &[u8], body: usize, end: usize) -> Result<bool> {
    let (Some(size), Some(count)) = (be_u32(b, body + 4), be_u32(b, body + 8)) else { return Ok(false) };
    let file = b.len() as u64;
    if u64::from(size) > file {
        return Err(malformed("sample"));
    }
    if size != 0 {
        return Ok(true);
    }
    let fits = table(b, body + 8, 4, end)?;
    let (sizes, _) = b.get(body + 12..end).unwrap_or_default().as_chunks::<4>();
    if sizes.iter().take(count as usize).any(|s| u64::from(u32::from_be_bytes(*s)) > file) {
        return Err(malformed("sample"));
    }
    Ok(fits)
}
