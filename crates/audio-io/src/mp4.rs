//! Size checks on MP4 atoms before symphonia reads them: it allocates whatever an atom, a sample
//! table or a sample claims, and a failed allocation aborts the process.

use crate::pcm::{be_u32, be_u64};
use crate::{AudioError, Result};

#[derive(Clone, Copy)]
enum Level {
    Top,
    Moov,
    Trak,
    Mdia,
    Minf,
    Stbl,
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
            (Level::Mdia, b"minf") => atoms(b, body, inner, Level::Minf)?,
            (Level::Minf, b"stbl") => atoms(b, body, inner, Level::Stbl)?,
            (Level::Meta, b"ilst") => atoms(b, body, inner, Level::Ilst)?,
            (Level::Ilst, _) => atoms(b, body, inner, Level::Tag)?,
            (Level::Stbl, b"stts" | b"co64") => table(b, body + 4, 8, inner)?,
            (Level::Stbl, b"stsc") => table(b, body + 4, 12, inner)?,
            (Level::Stbl, b"stco") => table(b, body + 4, 4, inner)?,
            (Level::Stbl, b"stsz") => sample_sizes(b, body, inner)?,
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

/// The part of a decoded MP4 audio track that the file's edit list keeps, in frames at the
/// track's sample rate. AAC streams start with encoder delay that only the edit list excludes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Trim {
    /// Frames to drop from the start of the decoded stream.
    pub skip: u64,
    /// Frames to keep after those, or all that remain.
    pub len: Option<u64>,
}

impl Trim {
    /// Total frames left of a decoded stream of `total` frames.
    pub(crate) fn frames(self, total: u64) -> u64 {
        let rest = total.saturating_sub(self.skip);
        self.len.map_or(rest, |n| n.min(rest))
    }

    /// Cut every channel down to the kept frames.
    pub(crate) fn apply(self, channels: &mut [Vec<f32>]) {
        for ch in channels {
            let skip = usize::try_from(self.skip).unwrap_or(usize::MAX).min(ch.len());
            ch.drain(..skip);
            if let Some(n) = self.len {
                ch.truncate(usize::try_from(n).unwrap_or(usize::MAX));
            }
        }
    }
}

/// Child atoms in `pos..end` as (kind, body start, atom end). Stops at the first header that
/// does not fit.
fn children(b: &[u8], mut pos: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let end = end.min(b.len());
    let mut out = Vec::new();
    while pos < end {
        let (Some(size), Some(kind)) = (be_u32(b, pos), crate::pcm::tag(b, pos + 4)) else { break };
        let (head, len) = match size {
            0 => (8, (end - pos) as u64),
            1 => (16, be_u64(b, pos + 8).unwrap_or(0)),
            n => (8, u64::from(n)),
        };
        let Some(atom_end) = usize::try_from(len).ok().filter(|_| len >= head).and_then(|n| pos.checked_add(n)).filter(|&n| n <= end) else { break };
        out.push((kind, pos + head as usize, atom_end));
        pos = atom_end;
    }
    out
}

/// The timescale of an `mvhd` or `mdhd` atom whose body starts at `body`.
fn timescale(b: &[u8], body: usize) -> Option<u64> {
    let at = if *b.get(body)? == 1 { body + 20 } else { body + 12 };
    be_u32(b, at).filter(|&t| t > 0).map(u64::from)
}

/// `value` units of a clock running at `from` Hz, as frames at `rate` Hz.
fn rescale(value: u64, rate: u64, from: u64) -> Option<u64> {
    u64::try_from(u128::from(value) * u128::from(rate) / u128::from(from)).ok()
}

/// The frames kept by the edit list of the first sound track of the MP4 file `b`, for a track
/// decoded at `rate` Hz. None without a usable edit list: several entries, an empty edit, or a
/// cut that keeps everything.
pub(crate) fn edit_trim(b: &[u8], rate: u32) -> Option<Trim> {
    let rate = u64::from(rate);
    let moov = children(b, 0, b.len()).into_iter().find(|c| &c.0 == b"moov")?;
    let parts = children(b, moov.1, moov.2);
    let movie_scale = timescale(b, parts.iter().find(|c| &c.0 == b"mvhd")?.1)?;
    for (_, trak, trak_end) in parts.iter().filter(|c| &c.0 == b"trak") {
        let kids = children(b, *trak, *trak_end);
        let mdia = kids.iter().find(|c| &c.0 == b"mdia")?;
        let media = children(b, mdia.1, mdia.2);
        let is_sound = media.iter().find(|c| &c.0 == b"hdlr").is_some_and(|h| crate::pcm::tag(b, h.1 + 8) == Some(*b"soun"));
        if !is_sound {
            continue;
        }
        let media_scale = timescale(b, media.iter().find(|c| &c.0 == b"mdhd")?.1)?;
        let edts = kids.iter().find(|c| &c.0 == b"edts")?;
        let elst = children(b, edts.1, edts.2).into_iter().find(|c| &c.0 == b"elst")?;
        let body = elst.1;
        let wide = *b.get(body)? == 1;
        if be_u32(b, body + 4)? != 1 {
            return None;
        }
        let (duration, start) = if wide {
            (be_u64(b, body + 8)?, i64::from_be_bytes(be_u64(b, body + 16)?.to_be_bytes()))
        } else {
            (u64::from(be_u32(b, body + 8)?), i64::from(i32::from_be_bytes(be_u32(b, body + 12)?.to_be_bytes())))
        };
        let skip = rescale(u64::try_from(start).ok()?, rate, media_scale)?;
        let len = if duration == 0 { None } else { Some(rescale(duration, rate, movie_scale)?) };
        return (skip > 0 || len.is_some()).then_some(Trim { skip, len });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn header(scale: u32) -> Vec<u8> {
        let mut v = vec![0u8; 12];
        v.extend_from_slice(&scale.to_be_bytes());
        v.extend_from_slice(&[0; 8]);
        v
    }

    fn file(movie_scale: u32, media_scale: u32, entries: &[(u32, i32)]) -> Vec<u8> {
        let mut elst = vec![0, 0, 0, 0];
        elst.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for &(d, t) in entries {
            elst.extend_from_slice(&d.to_be_bytes());
            elst.extend_from_slice(&t.to_be_bytes());
            elst.extend_from_slice(&[0, 1, 0, 0]);
        }
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(b"soun");
        let mdia = [atom(b"hdlr", &hdlr), atom(b"mdhd", &header(media_scale))].concat();
        let trak = [atom(b"mdia", &mdia), atom(b"edts", &atom(b"elst", &elst))].concat();
        let moov = [atom(b"mvhd", &header(movie_scale)), atom(b"trak", &trak)].concat();
        [atom(b"ftyp", b"M4A \0\0\0\0"), atom(b"moov", &moov)].concat()
    }

    #[test]
    fn edit_list_drops_priming_and_padding() {
        let t = edit_trim(&file(48_000, 48_000, &[(48_000, 1024)]), 48_000);
        assert_eq!(t, Some(Trim { skip: 1024, len: Some(48_000) }));
        assert_eq!(t.map(|t| t.frames(49_152)), Some(48_000));
    }

    #[test]
    fn edit_list_scales_movie_and_media_clocks() {
        // A 1000 Hz movie clock (as ffmpeg writes) and a 44.1 kHz media clock.
        let t = edit_trim(&file(1000, 44_100, &[(1000, 2112)]), 44_100);
        assert_eq!(t, Some(Trim { skip: 2112, len: Some(44_100) }));
    }

    #[test]
    fn empty_or_split_edit_lists_are_ignored() {
        assert_eq!(edit_trim(&file(1000, 48_000, &[(500, -1)]), 48_000), None);
        assert_eq!(edit_trim(&file(1000, 48_000, &[(500, 0), (500, 100)]), 48_000), None);
    }

    #[test]
    fn trim_cuts_every_channel() {
        let mut ch = vec![vec![1.0, 2.0, 3.0, 4.0], vec![5.0, 6.0, 7.0, 8.0]];
        Trim { skip: 1, len: Some(2) }.apply(&mut ch);
        assert_eq!(ch, vec![vec![2.0, 3.0], vec![6.0, 7.0]]);
    }
}
