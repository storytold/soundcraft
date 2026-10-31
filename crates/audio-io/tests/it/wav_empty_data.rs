//! A WAV whose data chunk is empty and followed by metadata decodes to no audio; a recording
//! that was never finalised (RIFF and data sizes left at 0) still yields what was written (#466).

use soundcraft_audio_io::{decode, probe};

fn fmt16(channels: u16, sr: u32) -> Vec<u8> {
    let align = channels * 2;
    let mut f = Vec::new();
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&channels.to_le_bytes());
    f.extend_from_slice(&sr.to_le_bytes());
    f.extend_from_slice(&(sr * u32::from(align)).to_le_bytes());
    f.extend_from_slice(&align.to_le_bytes());
    f.extend_from_slice(&16u16.to_le_bytes());
    f
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&(data.len() as u32).to_le_bytes());
    c.extend_from_slice(data);
    if data.len() % 2 == 1 {
        c.push(0);
    }
    c
}

/// A finished file: correct RIFF size, chunks as given.
fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut body = b"WAVE".to_vec();
    for c in chunks {
        body.extend_from_slice(c);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn list_info() -> Vec<u8> {
    let mut l = b"INFO".to_vec();
    l.extend_from_slice(&chunk(b"INAM", b"Silence"));
    chunk(b"LIST", &l)
}

#[test]
fn an_empty_data_chunk_followed_by_metadata_has_no_frames() {
    let file = riff(&[chunk(b"fmt ", &fmt16(2, 48_000)), chunk(b"data", &[]), list_info(), chunk(b"bext", &[0u8; 602])]);
    let info = probe(&file, Some("wav")).unwrap();
    assert_eq!(info.frames, 0, "the LIST and bext chunks are not samples");
    let (info, buf) = decode(&file, Some("wav")).unwrap();
    assert_eq!((info.frames, buf.frames()), (0, 0));
    // The same with the metadata first.
    let file = riff(&[chunk(b"fmt ", &fmt16(1, 44_100)), list_info(), chunk(b"data", &[])]);
    assert_eq!(probe(&file, Some("wav")).unwrap().frames, 0);
    // And with a single trailing chunk of odd size and no pad byte at the very end.
    let mut file = riff(&[chunk(b"fmt ", &fmt16(1, 44_100)), chunk(b"data", &[]), chunk(b"note", b"abc")]);
    file.pop();
    let riff_size = (file.len() - 8) as u32;
    file[4..8].copy_from_slice(&riff_size.to_le_bytes());
    assert_eq!(probe(&file, Some("wav")).unwrap().frames, 0);
}

#[test]
fn a_recording_that_was_never_finalised_still_gives_its_audio() {
    let samples: Vec<u8> = (0..400i16).flat_map(|i| (i * 40).to_le_bytes()).collect();
    // RIFF size and data size both left at 0, as a recorder writes them until it finishes.
    let mut file = riff(&[chunk(b"fmt ", &fmt16(2, 48_000)), chunk(b"data", &[])]);
    file.extend_from_slice(&samples);
    file[4..8].copy_from_slice(&0u32.to_le_bytes());
    let (info, buf) = decode(&file, Some("wav")).unwrap();
    assert_eq!(info.frames, 200);
    assert_eq!(buf.frames(), 200);
    // Even if the RIFF size is the file's, audio that is not a run of chunks is still audio.
    let mut file = riff(&[chunk(b"fmt ", &fmt16(2, 48_000)), chunk(b"data", &[])]);
    file.extend_from_slice(&samples);
    let riff_size = (file.len() - 8) as u32;
    file[4..8].copy_from_slice(&riff_size.to_le_bytes());
    assert_eq!(probe(&file, Some("wav")).unwrap().frames, 200);
}

#[test]
fn a_data_chunk_with_audio_is_unchanged() {
    let samples: Vec<u8> = (0..100i16).flat_map(|i| i.to_le_bytes()).collect();
    let file = riff(&[chunk(b"fmt ", &fmt16(1, 44_100)), chunk(b"data", &samples), list_info()]);
    assert_eq!(probe(&file, Some("wav")).unwrap().frames, 100);
}
