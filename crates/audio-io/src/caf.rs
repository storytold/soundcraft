//! Size checks on CAF chunks before symphonia reads them: it allocates whatever a magic cookie,
//! a packet table or a packet claims, and a failed allocation aborts the process.

use crate::pcm::{be_u16, be_u32, be_u64};
use crate::{AudioError, Result};

/// Refuse the CAF stream at `at` if its magic cookie, packet table or packets claim more bytes
/// than the file holds.
pub(crate) fn check(b: &[u8], at: usize) -> Result<()> {
    if be_u16(b, at + 4) == Some(1) { chunks(b, at + 8) } else { Ok(()) }
}

fn malformed(what: &str) -> AudioError {
    AudioError::Malformed(format!("CAF {what} is larger than the file"))
}

/// Walk the chunks the way the CAF reader reads them: it advances by the bytes it parses, not by
/// the declared size, and after a `data` chunk of unknown size it reads on into the audio.
fn chunks(b: &[u8], mut pos: usize) -> Result<()> {
    // Whether the packet table lists each packet's size and frame count (variable per packet).
    let (mut lists_sizes, mut lists_frames, mut described) = (false, false, false);
    while let (Some(kind), Some(size)) = (b.get(pos..pos + 4), be_u64(b, pos + 4)) {
        let (body, size) = (pos + 12, size as i64);
        let used = match kind {
            b"desc" if size == 32 => {
                lists_sizes = be_u32(b, body + 16) == Some(0);
                lists_frames = be_u32(b, body + 20) == Some(0);
                described = true;
                Some(32)
            }
            b"data" if size == -1 => Some(4),
            b"chan" if size >= 12 => be_u32(b, body + 8).map(|n| 12 + u64::from(n) * 20),
            b"pakt" if size >= 24 && described => packet_table(b, body, lists_sizes, lists_frames)?,
            b"kuki" if size >= 0 && size as u64 > b.len().saturating_sub(body) as u64 => return Err(malformed("magic cookie")),
            b"desc" | b"pakt" | b"chan" => None,
            _ => u64::try_from(size).ok(),
        };
        match used.filter(|_| described).and_then(|n| usize::try_from(n).ok()).and_then(|n| body.checked_add(n)) {
            Some(next) if next < b.len() => pos = next,
            _ => return Ok(()),
        }
    }
    Ok(())
}

/// Check a packet table's count and packet sizes; returns the bytes symphonia reads for it, or
/// None where symphonia stops. It reserves room for every packet before reading any, so a table
/// it would refuse after that, or that runs past the file, is refused first.
fn packet_table(b: &[u8], body: usize, lists_sizes: bool, lists_frames: bool) -> Result<Option<u64>> {
    let valid = |at: usize| be_u64(b, at).filter(|&n| n as i64 >= 0);
    let (Some(packets), Some(_)) = (valid(body), valid(body + 8)) else { return Ok(None) };
    let mut at = body + 24;
    if packets > b.len().saturating_sub(at) as u64 {
        return Err(malformed("packet table"));
    }
    if !lists_sizes && !lists_frames {
        return if packets == 0 { Ok(Some(24)) } else { Err(AudioError::Malformed("a constant bit rate CAF lists packets".into())) };
    }
    for _ in 0..packets {
        if lists_sizes {
            let Some((size, len)) = varint(b, at) else { return Err(malformed("packet table")) };
            if size > b.len() as u64 {
                return Err(malformed("packet"));
            }
            at += len;
        }
        if lists_frames {
            let Some((_, len)) = varint(b, at) else { return Err(malformed("packet table")) };
            at += len;
        }
    }
    Ok(Some((at - body) as u64))
}

/// A CAF variable-length integer at `at` and its length in bytes.
fn varint(b: &[u8], at: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for i in 0..9 {
        let x = *b.get(at + i)?;
        v |= u64::from(x & 0x7f);
        if x & 0x80 == 0 {
            return Some((v, i + 1));
        }
        v <<= 7;
    }
    None
}
