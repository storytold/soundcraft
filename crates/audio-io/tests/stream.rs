use soundcraft_audio_io::{AudioBuffer, AudioError, BitDepth, BwfInfo, EncodeOptions, FileEncoder, FileFormat, encode, encode_with_channel_mask};
use std::io::Cursor;

fn tone(channels: usize, frames: usize) -> AudioBuffer {
    let channels = (0..channels).map(|c| (0..frames).map(|i| ((i * (c + 3)) as f32 * 0.013).sin() * 1.2).collect()).collect();
    AudioBuffer { sample_rate: 44_100, channels }
}

fn streamed(buf: &AudioBuffer, opts: &EncodeOptions, mask: Option<u32>) -> Vec<u8> {
    let mut out = Cursor::new(b"pre".to_vec());
    out.set_position(3);
    let mut enc = FileEncoder::new(out, opts, buf.channels.len(), buf.sample_rate, mask).unwrap();
    let mut start = 0;
    for len in [1, 7, 4095, 4097, 333].into_iter().cycle() {
        if start >= buf.frames() {
            break;
        }
        let len = len.min(buf.frames() - start);
        let block: Vec<Vec<f32>> = buf.channels.iter().map(|c| c[start..start + len].to_vec()).collect();
        enc.write(&block, len).unwrap();
        start += len;
    }
    enc.finish().unwrap().into_inner()
}

#[test]
fn streamed_files_match_whole_buffer_encodes() {
    let bwf = BwfInfo { description: "take".into(), time_reference: 123_456, ..Default::default() };
    let cases = [
        (FileFormat::Wav, BitDepth::Int24, true, None, None),
        (FileFormat::Wav, BitDepth::Float32, false, None, None),
        (FileFormat::Wav, BitDepth::Int16, true, Some(bwf), Some(0x60F)),
        (FileFormat::Aiff, BitDepth::Int24, true, None, None),
        (FileFormat::Aiff, BitDepth::Float32, false, None, None),
        (FileFormat::Flac, BitDepth::Int16, true, None, None),
        (FileFormat::Flac, BitDepth::Int24, false, None, None),
    ];
    for (format, bit_depth, dither, bwf, mask) in cases {
        let opts = EncodeOptions { format, bit_depth, dither, bwf, bitrate_kbps: None };
        for channels in [1, 6] {
            let buf = tone(channels, 20_001);
            let whole = match mask {
                Some(m) => encode_with_channel_mask(&buf, &opts, m),
                None => encode(&buf, &opts),
            }
            .unwrap();
            assert_eq!(streamed(&buf, &opts, mask), [b"pre".as_slice(), &whole].concat(), "{opts:?}, {channels} channels");
        }
    }
}

#[test]
fn blocks_past_a_format_limit_are_refused_before_anything_is_written() {
    let buf = tone(2, 100);
    for (format, limit) in [(FileFormat::Aiff, 1usize << 30), (FileFormat::Flac, 1 << 36)] {
        let opts = EncodeOptions { format, bit_depth: BitDepth::Int16, dither: false, bwf: None, bitrate_kbps: None };
        let mut enc = FileEncoder::new(Cursor::new(Vec::new()), &opts, 2, buf.sample_rate, None).unwrap();
        enc.write(&buf.channels, 100).unwrap();
        assert!(enc.check_room(limit as u64 / 2).is_ok(), "{format:?}");
        assert!(matches!(enc.check_room(limit as u64), Err(AudioError::TooLarge(_))), "{format:?}");
        assert!(matches!(enc.write(&[], limit), Err(AudioError::TooLarge(_))), "{format:?}");
        assert_eq!(enc.finish().unwrap().into_inner(), encode(&buf, &opts).unwrap());
    }
}
