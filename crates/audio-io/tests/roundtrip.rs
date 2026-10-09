use soundcraft_audio_io::{
    AudioBuffer, BitDepth, BwfInfo, EncodeOptions, FileFormat, SampleFormat, decode, detect_format, encode, encode_wav_rf64, probe, wav_float_header,
};

fn sine(sr: u32, channels: usize, frames: usize) -> AudioBuffer {
    let mut b = AudioBuffer::new(sr, channels, frames);
    for (c, ch) in b.channels.iter_mut().enumerate() {
        let f = 220.0 * (c as f32 + 1.0);
        for (i, s) in ch.iter_mut().enumerate() {
            *s = 0.8 * (2.0 * std::f32::consts::PI * f * i as f32 / sr as f32 + c as f32).sin();
        }
    }
    b
}

fn tolerance(depth: BitDepth, dither: bool) -> f32 {
    let lsb = match depth {
        BitDepth::Int16 => 1.0 / 32768.0,
        BitDepth::Int24 => 1.0 / 8_388_608.0,
        BitDepth::Int32 => 1e-7,
        BitDepth::Float32 => 0.0,
    };
    lsb * if dither { 1.6 } else { 0.6 } + 1e-7
}

fn max_err(a: &AudioBuffer, b: &AudioBuffer) -> f32 {
    assert_eq!(a.num_channels(), b.num_channels());
    assert_eq!(a.frames(), b.frames());
    a.channels.iter().zip(&b.channels).flat_map(|(x, y)| x.iter().zip(y).map(|(p, q)| (p - q).abs())).fold(0.0, f32::max)
}

const DEPTHS: [BitDepth; 4] = [BitDepth::Int16, BitDepth::Int24, BitDepth::Int32, BitDepth::Float32];

fn expected_sf(d: BitDepth) -> SampleFormat {
    match d {
        BitDepth::Int16 => SampleFormat::Int16,
        BitDepth::Int24 => SampleFormat::Int24,
        BitDepth::Int32 => SampleFormat::Int32,
        BitDepth::Float32 => SampleFormat::Float32,
    }
}

#[test]
fn wav_and_aiff_roundtrip_all_depths_and_layouts() {
    for format in [FileFormat::Wav, FileFormat::Aiff] {
        for depth in DEPTHS {
            for channels in [1, 2, 6] {
                for dither in [false, true] {
                    let src = sine(48_000, channels, 4801);
                    let opts = EncodeOptions { format, bit_depth: depth, dither, bwf: None };
                    let bytes = encode(&src, &opts).unwrap();
                    assert_eq!(detect_format(&bytes, None), format);
                    let info = probe(&bytes, None).unwrap();
                    assert_eq!(info.format, format);
                    assert_eq!(info.sample_rate, 48_000);
                    assert_eq!(info.channels as usize, channels);
                    assert_eq!(info.frames, 4801);
                    assert_eq!(info.sample_format, expected_sf(depth));
                    let (dinfo, out) = decode(&bytes, None).unwrap();
                    assert_eq!(dinfo, info);
                    assert_eq!(out.sample_rate, 48_000);
                    let err = max_err(&src, &out);
                    assert!(err <= tolerance(depth, dither), "{format:?} {depth:?} {channels}ch dither={dither}: err {err}");
                }
            }
        }
    }
}

#[test]
fn flac_roundtrip_is_lossless_vs_wav() {
    for depth in [BitDepth::Int16, BitDepth::Int24, BitDepth::Int32] {
        for channels in [1, 2, 6] {
            // Spans several blocks plus a short final block.
            let src = sine(44_100, channels, 4096 * 2 + 17);
            let wav = encode(&src, &EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: None }).unwrap();
            let flac = encode(&src, &EncodeOptions { format: FileFormat::Flac, bit_depth: depth, dither: false, bwf: None }).unwrap();
            assert_eq!(detect_format(&flac, None), FileFormat::Flac);
            let info = probe(&flac, None).unwrap();
            assert_eq!(info.format, FileFormat::Flac);
            assert_eq!((info.sample_rate, info.channels as usize, info.frames), (44_100, channels, src.frames() as u64));
            assert_eq!(info.sample_format, expected_sf(depth));
            let (_, from_wav) = decode(&wav, None).unwrap();
            let (finfo, from_flac) = decode(&flac, None).unwrap();
            assert_eq!(finfo.frames, src.frames() as u64);
            assert_eq!(from_flac, from_wav, "{depth:?} {channels}ch");
            if depth != BitDepth::Int32 {
                assert!(flac.len() < wav.len(), "{depth:?} {channels}ch: flac {} >= wav {}", flac.len(), wav.len());
            }
        }
    }
}

#[test]
fn flac_handles_silence_noise_full_scale_and_odd_rates() {
    let frames = 10_000;
    let mut b = AudioBuffer::new(37_123, 3, frames);
    let mut seed = 1u32;
    for i in 0..frames {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        b.channels[1][i] = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0; // white noise
        b.channels[2][i] = if i % 2 == 0 { 1.5 } else { -1.5 }; // clipped full-scale square
    }
    for depth in [BitDepth::Int16, BitDepth::Int24, BitDepth::Int32] {
        for dither in [false, true] {
            let opts = EncodeOptions { format: FileFormat::Flac, bit_depth: depth, dither, bwf: None };
            let flac = encode(&b, &opts).unwrap();
            let (info, out) = decode(&flac, None).unwrap();
            assert_eq!(info.sample_rate, 37_123);
            let wav = encode(&b, &EncodeOptions { format: FileFormat::Wav, ..opts }).unwrap();
            let (_, expect) = decode(&wav, None).unwrap();
            assert_eq!(out, expect, "{depth:?} dither={dither}");
        }
    }
}

#[test]
fn flac_rejects_unsupported_layouts() {
    let opts = EncodeOptions { format: FileFormat::Flac, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    assert!(encode(&sine(48_000, 2, 10), &opts).is_err());
    let opts = EncodeOptions { bit_depth: BitDepth::Int16, ..opts };
    assert!(encode(&sine(48_000, 9, 10), &opts).is_err());
    assert!(encode(&sine(700_000, 1, 10), &opts).is_err());
    // Tiny buffers still encode and decode.
    let one = sine(48_000, 2, 1);
    let (_, out) = decode(&encode(&one, &opts).unwrap(), None).unwrap();
    assert_eq!(out.frames(), 1);
}

#[test]
fn bwf_roundtrip() {
    let bwf = BwfInfo {
        description: "Scene 12 take 3".into(),
        originator: "SoundCraft".into(),
        originator_reference: "SC-0001".into(),
        origination_date: "2026-10-06".into(),
        origination_time: "12:34:56".into(),
        time_reference: 0x1_2345_6789, // > 32 bits
    };
    let src = sine(96_000, 2, 1000);
    for depth in DEPTHS {
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: Some(bwf.clone()) };
        let bytes = encode(&src, &opts).unwrap();
        let info = probe(&bytes, None).unwrap();
        assert_eq!(info.bwf.as_ref(), Some(&bwf));
        let (dinfo, _) = decode(&bytes, None).unwrap();
        assert_eq!(dinfo.bwf, Some(bwf.clone()));
        // RF64 layout reads back identically.
        let rf64 = encode_wav_rf64(&src, &opts).unwrap();
        assert_eq!(&rf64[..4], b"RF64");
        let (rinfo, rbuf) = decode(&rf64, None).unwrap();
        assert_eq!(rinfo.bwf, Some(bwf.clone()));
        assert_eq!(rinfo.frames, 1000);
        assert_eq!(rbuf, decode(&bytes, None).unwrap().1);
    }
    // Overlong strings are truncated to the field width, not an error.
    let long = BwfInfo { description: "x".repeat(400), ..BwfInfo::default() };
    let bytes = encode(&src, &EncodeOptions { bwf: Some(long), ..EncodeOptions::default() }).unwrap();
    assert_eq!(probe(&bytes, None).unwrap().bwf.unwrap().description.len(), 256);
}

#[test]
fn wav_header_layout() {
    let b = encode(&sine(44_100, 2, 10), &EncodeOptions { bit_depth: BitDepth::Int16, ..EncodeOptions::default() }).unwrap();
    // Plain PCM for 16-bit stereo; extensible otherwise.
    let fmt = b.windows(4).position(|w| w == b"fmt ").unwrap();
    assert_eq!(u16::from_le_bytes([b[fmt + 8], b[fmt + 9]]), 1);
    let b = encode(&sine(44_100, 2, 10), &EncodeOptions::default()).unwrap();
    let fmt = b.windows(4).position(|w| w == b"fmt ").unwrap();
    assert_eq!(u16::from_le_bytes([b[fmt + 8], b[fmt + 9]]), 0xFFFE);
    let riff = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
    assert_eq!(riff + 8, b.len());
    // Odd data length (24-bit mono, odd frames) gets a pad byte.
    let b = encode(&sine(44_100, 1, 3), &EncodeOptions::default()).unwrap();
    assert_eq!(b.len() % 2, 0);
    assert_eq!(decode(&b, None).unwrap().1.frames(), 3);
}

#[test]
fn aiff_float_is_aifc() {
    let b =
        encode(&sine(44_100, 2, 10), &EncodeOptions { format: FileFormat::Aiff, bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
    assert_eq!(&b[8..12], b"AIFC");
    assert!(b.windows(4).any(|w| w == b"fl32"));
    let b = encode(&sine(44_100, 2, 10), &EncodeOptions { format: FileFormat::Aiff, ..EncodeOptions::default() }).unwrap();
    assert_eq!(&b[8..12], b"AIFF");
    let form = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as usize;
    assert_eq!(form + 8, b.len());
}

#[test]
fn unusual_sample_rates_roundtrip() {
    for sr in [1, 8_000, 11_025, 22_050, 44_100, 88_200, 176_400, 192_000, 352_800, 384_000, 768_000] {
        for format in [FileFormat::Wav, FileFormat::Aiff] {
            let bytes = encode(&sine(sr, 1, 8), &EncodeOptions { format, ..EncodeOptions::default() }).unwrap();
            assert_eq!(probe(&bytes, None).unwrap().sample_rate, sr, "{format:?}");
        }
    }
}

#[test]
fn encode_rejects_bad_buffers_and_formats() {
    assert!(encode(&AudioBuffer::default(), &EncodeOptions::default()).is_err());
    assert!(encode(&AudioBuffer::new(0, 2, 10), &EncodeOptions::default()).is_err());
    for f in [FileFormat::Mp3, FileFormat::Ogg, FileFormat::Aac, FileFormat::Alac, FileFormat::Caf, FileFormat::Other] {
        assert!(encode(&sine(48_000, 1, 10), &EncodeOptions { format: f, ..EncodeOptions::default() }).is_err());
    }
    // Non-finite and out-of-range samples are clamped, not errors.
    let mut b = AudioBuffer::new(48_000, 1, 4);
    b.channels[0] = vec![f32::NAN, f32::INFINITY, -5.0, 0.5];
    for format in [FileFormat::Wav, FileFormat::Aiff, FileFormat::Flac] {
        for depth in DEPTHS {
            if format == FileFormat::Flac && depth == BitDepth::Float32 {
                continue;
            }
            let bytes = encode(&b, &EncodeOptions { format, bit_depth: depth, ..EncodeOptions::default() }).unwrap();
            let (_, out) = decode(&bytes, None).unwrap();
            assert_eq!(out.channels[0][0], 0.0);
            assert!(out.channels[0][1] > 0.99 && out.channels[0][1] <= 1.0);
            assert_eq!(out.channels[0][2], -1.0);
        }
    }
    // Ragged buffers encode the common length.
    let ragged = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 10], vec![0.2; 7]] };
    let (_, out) = decode(&encode(&ragged, &EncodeOptions::default()).unwrap(), None).unwrap();
    assert_eq!(out.frames(), 7);
}

#[test]
fn audio_buffer_helpers() {
    let b = AudioBuffer::from_interleaved(48_000, 2, &[0.1, -0.2, 0.3, -0.4, 0.5]);
    assert_eq!(b.frames(), 2);
    assert_eq!(b.num_channels(), 2);
    assert_eq!(b.interleaved(), vec![0.1, -0.2, 0.3, -0.4]);
    assert!((b.peak() - 0.4).abs() < 1e-7);
    assert!((b.duration_secs() - 2.0 / 48_000.0).abs() < 1e-12);
    assert_eq!(AudioBuffer::from_interleaved(1, 0, &[1.0]).num_channels(), 0);
    assert_eq!(AudioBuffer::new(0, 1, 5).duration_secs(), 0.0);
    assert_eq!(AudioBuffer::default().frames(), 0);
}

#[test]
fn serde_types_roundtrip() {
    let opts = EncodeOptions { bwf: Some(BwfInfo { time_reference: 42, ..BwfInfo::default() }), ..EncodeOptions::default() };
    let s = serde_json::to_string(&opts).unwrap();
    assert_eq!(serde_json::from_str::<EncodeOptions>(&s).unwrap(), opts);
    let info = probe(&encode(&sine(48_000, 1, 4), &opts).unwrap(), None).unwrap();
    let s = serde_json::to_string(&info).unwrap();
    assert_eq!(serde_json::from_str::<soundcraft_audio_io::AudioInfo>(&s).unwrap(), info);
}

#[test]
fn multichannel_wav_speaker_masks() {
    let buf = |n: usize| AudioBuffer { sample_rate: 48_000, channels: (0..n).map(|c| vec![0.01 * c as f32; 64]).collect() };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None };
    for (n, mask) in [(6usize, 0x3Fu32), (8, 0x63F), (12, 0x2_D63F)] {
        let bytes = soundcraft_audio_io::encode(&buf(n), &opts).unwrap();
        assert_eq!(soundcraft_audio_io::wav_channel_mask(&bytes), Some(mask), "{n} channels");
        assert_eq!(soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap().1.channels.len(), n);
    }
    // Explicit masks: 7.0 (sides + rears, no LFE) and Ambisonics (no positions).
    let bytes = soundcraft_audio_io::encode_with_channel_mask(&buf(7), &opts, 0x637).unwrap();
    assert_eq!(soundcraft_audio_io::wav_channel_mask(&bytes), Some(0x637));
    let bytes = soundcraft_audio_io::encode_with_channel_mask(&buf(4), &opts, 0).unwrap();
    assert_eq!(soundcraft_audio_io::wav_channel_mask(&bytes), Some(0));
    // Plain 16-bit stereo stays a plain PCM header.
    let o16 = EncodeOptions { bit_depth: BitDepth::Int16, ..opts };
    let bytes = soundcraft_audio_io::encode_with_channel_mask(&buf(2), &o16, 0x3).unwrap();
    assert_eq!(soundcraft_audio_io::wav_channel_mask(&bytes), None);
}

#[test]
fn float_wav_header_is_rewritten_in_place_as_the_file_grows() {
    let take = sine(48_000, 2, 1_000);
    let mut file = wav_float_header(2, 48_000, 0).unwrap();
    let len = file.len();
    file.extend(take.interleaved().iter().flat_map(|s| s.to_le_bytes()));
    assert_eq!(decode(&file, None).unwrap().1.frames(), 1_000);
    file[..len].copy_from_slice(&wav_float_header(2, 48_000, 600).unwrap());
    assert_eq!(decode(&file, None).unwrap().1.frames(), 600);
    file[..len].copy_from_slice(&wav_float_header(2, 48_000, 1_000).unwrap());
    assert_eq!(file, encode(&take, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap());
    let big = wav_float_header(2, 48_000, 600_000_000).unwrap();
    assert_eq!((&big[..4], big.len()), (&b"RF64"[..], len));
    assert!(wav_float_header(0, 48_000, 0).is_err());
}
