use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileEncoder, FileFormat, SampleFormat, decode, encode};
use std::io::Cursor;

fn encoded(audio: &AudioBuffer, options: &EncodeOptions, streaming: bool) -> Vec<u8> {
    if !streaming {
        return encode(audio, options).unwrap();
    }
    let mut writer = FileEncoder::new(Cursor::new(Vec::new()), options, audio.num_channels(), audio.sample_rate, None).unwrap();
    // Cross the writer's 4096-frame scratch boundary, then write a short final block.
    for start in (0..audio.frames()).step_by(4097) {
        let end = (start + 4097).min(audio.frames());
        let block: Vec<Vec<f32>> = audio.channels.iter().map(|channel| channel[start..end].to_vec()).collect();
        writer.write(&block, end - start).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn assert_float_headroom(streaming: bool) {
    let samples = [
        -f32::MAX,
        -2.0,
        -1.25,
        -1.0,
        -0.5,
        -f32::MIN_POSITIVE,
        -f32::from_bits(1),
        -0.0,
        0.0,
        f32::from_bits(1),
        f32::MIN_POSITIVE,
        0.5,
        1.0,
        1.25,
        2.0,
        f32::MAX,
    ];
    for format in [FileFormat::Wav, FileFormat::Aiff] {
        for dither in [false, true] {
            for channels in [1, 2] {
                let audio = AudioBuffer {
                    sample_rate: 48_000,
                    channels: (0..channels).map(|c| samples.iter().copied().cycle().skip(c).take(4103).collect()).collect(),
                };
                let options = EncodeOptions { format, bit_depth: BitDepth::Float32, dither, bwf: None };
                let bytes = encoded(&audio, &options, streaming);
                let pcm: Vec<u8> =
                    audio.interleaved().iter().flat_map(|v| if format == FileFormat::Wav { v.to_le_bytes() } else { v.to_be_bytes() }).collect();
                assert!(bytes.ends_with(&pcm), "{format:?}, dither={dither}: stored sample bytes changed");
                let (info, decoded) = decode(&bytes, None).unwrap();
                assert_eq!(info.sample_format, SampleFormat::Float32);
                assert_eq!((decoded.sample_rate, decoded.num_channels(), decoded.frames()), (48_000, channels, 4103));
                for (channel, (actual, expected)) in decoded.channels.iter().zip(&audio.channels).enumerate() {
                    for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                        assert_eq!(actual.to_bits(), expected.to_bits(), "{format:?}, dither={dither}, channel={channel}, frame={frame}");
                    }
                }
            }
        }
    }
}

#[test]
fn whole_file_float32_preserves_finite_headroom() {
    assert_float_headroom(false);
}

#[test]
fn streaming_float32_preserves_finite_headroom() {
    assert_float_headroom(true);
}

#[test]
fn float32_keeps_non_finite_sample_handling() {
    let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![f32::NAN, f32::NEG_INFINITY, f32::INFINITY]] };
    let expected = [0.0_f32, -1.0, 1.0];
    for format in [FileFormat::Wav, FileFormat::Aiff] {
        for streaming in [false, true] {
            let options = EncodeOptions { format, bit_depth: BitDepth::Float32, dither: false, bwf: None };
            let bytes = encoded(&audio, &options, streaming);
            // Inspect stored samples too: the decoder itself replaces non-finite values.
            let pcm: Vec<u8> = expected.iter().flat_map(|v| if format == FileFormat::Wav { v.to_le_bytes() } else { v.to_be_bytes() }).collect();
            assert!(bytes.ends_with(&pcm), "{format:?}, streaming={streaming}");
            assert_eq!(decode(&bytes, None).unwrap().1.channels[0], expected);
        }
    }
}

#[test]
fn integer_pcm_still_clips_at_full_scale() {
    let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![-2.0, -1.25, -1.0, -0.5, 0.0, 0.5, 1.0, 1.25, 2.0]] };
    for (depth, max) in [(BitDepth::Int16, 32767.0 / 32768.0), (BitDepth::Int24, 8_388_607.0 / 8_388_608.0), (BitDepth::Int32, 1.0)] {
        for format in [FileFormat::Wav, FileFormat::Aiff, FileFormat::Flac] {
            for streaming in [false, true] {
                let options = EncodeOptions { format, bit_depth: depth, dither: false, bwf: None };
                let bytes = encoded(&audio, &options, streaming);
                let (_, decoded) = decode(&bytes, None).unwrap();
                assert_eq!(decoded.channels[0], [-1.0, -1.0, -1.0, -0.5, 0.0, 0.5, max, max, max], "{format:?} {depth:?}, streaming={streaming}");
            }
        }
    }
}
