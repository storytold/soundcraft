//! Windowed reads from a file-backed PCM/float WAV reader.

use soundcraft_audio_io::{AudioBuffer, BitDepth, DiskWavReader, EncodeOptions, FileFormat, decode, encode};
use std::io::{Cursor, Write};

fn tone(channels: usize, frames: usize) -> AudioBuffer {
    let mut buf = AudioBuffer::new(48_000, channels, frames);
    for (c, ch) in buf.channels.iter_mut().enumerate() {
        for (i, s) in ch.iter_mut().enumerate() {
            let phase = (i * (c + 1)) as f32 * 0.07;
            *s = (phase.sin() * 0.5).clamp(-1.0, 1.0);
        }
    }
    buf
}

fn wav(depth: BitDepth, channels: usize) -> (AudioBuffer, Vec<u8>) {
    let src = tone(channels, 32);
    let bytes = encode(&src, &EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: None }).expect("encode");
    let (_info, decoded) = decode(&bytes, Some("wav")).expect("decode");
    (decoded, bytes)
}

fn assert_window(depth: BitDepth) {
    let (decoded, bytes) = wav(depth, 2);
    let mut reader = DiskWavReader::from_reader(Cursor::new(bytes)).expect("open");
    assert_eq!(reader.info().frames, decoded.frames() as u64);
    let window = reader.read_frames(4, 7).expect("window");
    assert_eq!(window.frames(), 7);
    for (got, full) in window.channels.iter().zip(&decoded.channels) {
        assert_eq!(got.as_slice(), &full[4..11]);
    }
}

#[test]
fn windows_match_the_in_memory_decoder_for_pcm_and_float() {
    assert_window(BitDepth::Int16);
    assert_window(BitDepth::Int24);
    assert_window(BitDepth::Float32);
}

#[test]
fn reads_at_and_past_the_end_are_empty_and_do_not_panic() {
    let (decoded, bytes) = wav(BitDepth::Int16, 1);
    let frames = decoded.frames() as u64;
    let mut reader = DiskWavReader::from_reader(Cursor::new(bytes)).expect("open");
    let at_end = reader.read_frames(frames, 8).expect("at end");
    assert_eq!(at_end.frames(), 0);
    assert_eq!(at_end.num_channels(), 1);
    let past = reader.read_frames(u64::MAX, usize::MAX).expect("past end");
    assert_eq!(past.frames(), 0);
    let last = reader.read_frames(frames - 1, 50).expect("last frame");
    assert_eq!(last.channels[0].as_slice(), &decoded.channels[0][decoded.frames() - 1..]);
}

#[test]
fn a_list_chunk_after_data_is_not_sample_data() {
    let (decoded, mut bytes) = wav(BitDepth::Int24, 2);
    let marker = [0x7Fu8; 64];
    bytes.extend_from_slice(b"LIST");
    bytes.extend_from_slice(&(marker.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&marker);
    let mut reader = DiskWavReader::from_reader(Cursor::new(bytes)).expect("open");
    assert_eq!(reader.info().frames, decoded.frames() as u64);
    let all = reader.read_frames(0, decoded.frames() + 16).expect("read");
    assert_eq!(all.channels, decoded.channels);
}

#[test]
fn a_data_chunk_past_one_mibibyte_still_opens() {
    let (_decoded, bytes) = wav(BitDepth::Int16, 1);
    let data_at = bytes.windows(4).position(|w| w == b"data").expect("data");
    let junk = 1024 * 1024 + 64;
    let mut file = Vec::new();
    file.extend_from_slice(&bytes[..data_at]);
    file.extend_from_slice(b"JUNK");
    file.extend_from_slice(&(junk as u32).to_le_bytes());
    file.extend(std::iter::repeat_n(b'x', junk));
    if junk % 2 == 1 {
        file.push(0);
    }
    file.extend_from_slice(&bytes[data_at..]);
    let mut reader = DiskWavReader::from_reader(Cursor::new(file)).expect("open");
    let got = reader.read_frames(0, 8).expect("read");
    let (_info, decoded) = decode(&bytes, Some("wav")).expect("decode");
    assert_eq!(got.channels[0].as_slice(), &decoded.channels[0][..8]);
}

#[test]
fn a_truncated_file_is_an_error() {
    let (_decoded, bytes) = wav(BitDepth::Float32, 1);
    let cut = &bytes[..20];
    let err = DiskWavReader::from_reader(Cursor::new(cut.to_vec()));
    assert!(err.is_err(), "a header cut off before the data chunk must not panic");
}

#[test]
fn open_reads_a_file_on_disk() {
    let (decoded, bytes) = wav(BitDepth::Int16, 2);
    let path = std::env::temp_dir().join(format!("soundcraft-disk-wav-{}-{}.wav", std::process::id(), line!()));
    std::fs::File::create(&path).expect("create").write_all(&bytes).expect("write");
    let mut reader = DiskWavReader::open(&path).expect("open");
    let got = reader.read_frames(0, decoded.frames()).expect("read");
    let _ = std::fs::remove_file(&path);
    assert_eq!(got.channels, decoded.channels);
}
