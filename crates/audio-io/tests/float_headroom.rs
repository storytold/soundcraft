use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, SampleFormat, decode, encode, encode_wav_rf64};

fn payload(bytes: &[u8], format: FileFormat) -> &[u8] {
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let tag = &bytes[pos..pos + 4];
        let size: [u8; 4] = bytes[pos + 4..pos + 8].try_into().unwrap();
        let size = if format == FileFormat::Wav { u32::from_le_bytes(size) } else { u32::from_be_bytes(size) };
        let start = pos + 8;
        // RF64's data length is a sentinel; our small fixture's data is the final chunk.
        let len = if size == u32::MAX { bytes.len() - start } else { size as usize };
        let body = &bytes[start..start + len];
        if format == FileFormat::Wav && tag == b"data" {
            return body;
        }
        if format == FileFormat::Aiff && tag == b"SSND" {
            let offset = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
            return &body[8 + offset..];
        }
        pos = start + len + (len & 1);
    }
    panic!("missing PCM data chunk")
}

fn float_payload(buf: &AudioBuffer, format: FileFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    for frame in 0..buf.frames() {
        for channel in &buf.channels {
            bytes.extend_from_slice(&if format == FileFormat::Wav { channel[frame].to_le_bytes() } else { channel[frame].to_be_bytes() });
        }
    }
    bytes
}

fn assert_sample_bits(actual: &AudioBuffer, expected: &AudioBuffer) {
    assert_eq!(actual.sample_rate, expected.sample_rate);
    assert_eq!(actual.channels.len(), expected.channels.len());
    for (channel, (actual, expected)) in actual.channels.iter().zip(&expected.channels).enumerate() {
        let actual: Vec<u32> = actual.iter().map(|sample| sample.to_bits()).collect();
        let expected: Vec<u32> = expected.iter().map(|sample| sample.to_bits()).collect();
        assert_eq!(actual, expected, "channel {channel}");
    }
}

#[test]
fn float_containers_preserve_finite_bits_and_distinct_channel_order() {
    let values = [
        1.25,
        -2.5,
        32_768.0,
        -65_536.0,
        f32::MAX,
        -f32::MAX,
        1.0,
        -1.0,
        0.0,
        -0.0,
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
        f32::from_bits(1),
        f32::from_bits(0x8000_0001),
        f32::from_bits(0x007f_ffff),
        f32::from_bits(0x807f_ffff),
        0.123_456_79,
        -0.765_432_1,
    ];
    let source = AudioBuffer {
        sample_rate: 96_000,
        channels: (0..6).map(|channel| (0..values.len()).map(|frame| values[(frame + channel) % values.len()]).collect()).collect(),
    };
    for (format, rf64) in [(FileFormat::Wav, false), (FileFormat::Wav, true), (FileFormat::Aiff, false)] {
        for dither in [false, true] {
            let opts = EncodeOptions { format, bit_depth: BitDepth::Float32, dither, bwf: None };
            let bytes = if rf64 { encode_wav_rf64(&source, &opts) } else { encode(&source, &opts) }.unwrap();
            assert_eq!(payload(&bytes, format), float_payload(&source, format), "{format:?} RF64={rf64} dither={dither}");
            let (info, decoded) = decode(&bytes, None).unwrap();
            assert_eq!(info.sample_format, SampleFormat::Float32);
            assert_eq!((info.channels, info.frames), (6, values.len() as u64));
            assert_sample_bits(&decoded, &source);
        }
    }
}

#[test]
fn float_payloads_silence_nonfinite_samples_without_clipping_finite_neighbors() {
    let values = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(0xffc0_1234), 1.25, -2.5, -0.0, f32::from_bits(1)];
    let expected = vec![0.0, 0.0, 0.0, 0.0, 1.25, -2.5, -0.0, f32::from_bits(1)];
    let source = AudioBuffer { sample_rate: 48_000, channels: vec![values.clone(), values.into_iter().rev().collect()] };
    let expected = AudioBuffer { sample_rate: 48_000, channels: vec![expected.clone(), expected.into_iter().rev().collect()] };
    for format in [FileFormat::Wav, FileFormat::Aiff] {
        let opts = EncodeOptions { format, bit_depth: BitDepth::Float32, dither: false, bwf: None };
        let bytes = encode(&source, &opts).unwrap();
        assert_eq!(payload(&bytes, format), float_payload(&expected, format));
        assert_sample_bits(&decode(&bytes, None).unwrap().1, &expected);
        // An external float file may still contain NaN/Inf: the reader applies the same policy.
        let mut external = bytes;
        let samples = float_payload(&source, format);
        let start = external.len() - samples.len();
        external[start..].copy_from_slice(&samples);
        assert_sample_bits(&decode(&external, None).unwrap().1, &expected);
    }
}

#[test]
fn integer_clipping_and_dither_keep_their_existing_policy() {
    let source = AudioBuffer { sample_rate: 48_000, channels: vec![vec![3.0, -4.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5]] };
    let clipped = AudioBuffer { sample_rate: 48_000, channels: vec![vec![1.0, -1.0, 0.0, 1.0, -1.0, 0.5]] };
    for (depth, bits) in [(BitDepth::Int16, 16), (BitDepth::Int24, 24), (BitDepth::Int32, 32)] {
        for format in [FileFormat::Wav, FileFormat::Aiff, FileFormat::Flac] {
            for dither in [false, true] {
                let opts = EncodeOptions { format, bit_depth: depth, dither, bwf: None };
                let bytes = encode(&source, &opts).unwrap();
                assert_eq!(bytes, encode(&clipped, &opts).unwrap(), "{format:?} {depth:?} dither={dither}");
                if !dither && format != FileFormat::Flac {
                    let scale = 1i64 << (bits - 1);
                    let codes = [scale - 1, -scale, 0, scale - 1, -scale, scale / 2];
                    let width = bits / 8;
                    let expected: Vec<u8> = codes
                        .iter()
                        .flat_map(|&code| {
                            let bytes = if format == FileFormat::Wav { (code as i32).to_le_bytes() } else { (code as i32).to_be_bytes() };
                            if format == FileFormat::Wav { bytes[..width].to_vec() } else { bytes[4 - width..].to_vec() }
                        })
                        .collect();
                    assert_eq!(payload(&bytes, format), expected);
                }
            }
        }
    }
}
