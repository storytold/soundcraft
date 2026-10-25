//! Hostile inputs must produce `Err` (or a best-effort `Ok`), never a panic.

use proptest::prelude::*;
use soundcraft_audio_io::{AudioBuffer, AudioError, BitDepth, EncodeOptions, FileFormat, SampleFormat, decode, detect_format, encode, probe};

fn small(format: FileFormat, depth: BitDepth, channels: usize) -> Vec<u8> {
    let mut b = AudioBuffer::new(44_100, channels, 300);
    for ch in &mut b.channels {
        for (i, s) in ch.iter_mut().enumerate() {
            *s = ((i as f32) * 0.1).sin() * 0.5;
        }
    }
    encode(&b, &EncodeOptions { format, bit_depth: depth, dither: false, bwf: None }).unwrap()
}

/// An ID3v2.4 header whose syncsafe size field declares `size` bytes of tag body.
fn id3_header(size: usize) -> [u8; 10] {
    let size = size as u32;
    [b'I', b'D', b'3', 4, 0, 0, ((size >> 21) & 0x7F) as u8, ((size >> 14) & 0x7F) as u8, ((size >> 7) & 0x7F) as u8, (size & 0x7F) as u8]
}

/// Wrap `body` in an ID3v2.4 tag.
fn id3_tag(body: &[u8]) -> Vec<u8> {
    [&id3_header(body.len())[..], body].concat()
}

/// Little helper for building RIFF files by hand.
fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = b"WAVE".to_vec();
    for (id, data) in chunks {
        body.extend_from_slice(*id);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        if data.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn fmt(tag: u16, channels: u16, sr: u32, bits: u16) -> Vec<u8> {
    let align = channels * bits.div_ceil(8);
    let mut f = Vec::new();
    f.extend_from_slice(&tag.to_le_bytes());
    f.extend_from_slice(&channels.to_le_bytes());
    f.extend_from_slice(&sr.to_le_bytes());
    f.extend_from_slice(&(sr * u32::from(align)).to_le_bytes());
    f.extend_from_slice(&align.to_le_bytes());
    f.extend_from_slice(&bits.to_le_bytes());
    f
}

#[test]
fn every_truncation_is_handled() {
    let files = [
        small(FileFormat::Wav, BitDepth::Int16, 2),
        small(FileFormat::Wav, BitDepth::Float32, 6),
        small(FileFormat::Aiff, BitDepth::Int24, 1),
        small(FileFormat::Aiff, BitDepth::Float32, 2),
        small(FileFormat::Flac, BitDepth::Int16, 2),
        small(FileFormat::Flac, BitDepth::Int24, 1),
    ];
    for file in &files {
        for n in 0..file.len() {
            let cut = &file[..n];
            let _ = probe(cut, None);
            if let Ok((info, buf)) = decode(cut, None) {
                assert_eq!(buf.num_channels(), info.channels as usize);
                assert_eq!(buf.frames() as u64, info.frames);
            }
        }
    }
}

#[test]
fn truncated_data_chunk_yields_partial_audio() {
    let file = small(FileFormat::Wav, BitDepth::Int16, 2);
    let (info, buf) = decode(&file[..file.len() - 400], None).unwrap();
    assert_eq!(info.frames, 200);
    assert_eq!(buf.frames(), 200);
}

#[test]
fn every_single_byte_corruption_is_handled() {
    for file in [small(FileFormat::Wav, BitDepth::Int24, 2), small(FileFormat::Aiff, BitDepth::Int16, 2), small(FileFormat::Flac, BitDepth::Int16, 1)]
    {
        for i in 0..file.len().min(200) {
            for v in [0x00, 0xFF, 0x7F, 0x80] {
                let mut f = file.clone();
                f[i] = v;
                let _ = probe(&f, None);
                let _ = decode(&f, None);
            }
        }
    }
}

#[test]
fn zero_channels_is_malformed() {
    let wav = riff(&[(b"fmt ", fmt(1, 0, 44_100, 16)), (b"data", vec![0; 16])]);
    assert!(matches!(decode(&wav, None), Err(AudioError::Malformed(_))));
    assert!(probe(&wav, None).is_err());
    let mut aiff = small(FileFormat::Aiff, BitDepth::Int16, 1);
    let comm = aiff.windows(4).position(|w| w == b"COMM").unwrap();
    aiff[comm + 8] = 0;
    aiff[comm + 9] = 0;
    assert!(matches!(decode(&aiff, None), Err(AudioError::Malformed(_))));
}

#[test]
fn zero_sample_rate_is_malformed() {
    let wav = riff(&[(b"fmt ", fmt(1, 2, 0, 16)), (b"data", vec![0; 16])]);
    assert!(decode(&wav, None).is_err());
    let mut aiff = small(FileFormat::Aiff, BitDepth::Int16, 1);
    let comm = aiff.windows(4).position(|w| w == b"COMM").unwrap();
    for b in &mut aiff[comm + 16..comm + 26] {
        *b = 0;
    }
    assert!(decode(&aiff, None).is_err());
    // Infinite exponent.
    aiff[comm + 16] = 0x7F;
    aiff[comm + 17] = 0xFF;
    assert!(decode(&aiff, None).is_err());
}

#[test]
fn absurd_chunk_sizes() {
    // fmt chunk claims 4 GiB.
    let mut wav = riff(&[(b"fmt ", fmt(1, 2, 44_100, 16)), (b"data", vec![0; 16])]);
    wav[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&wav, None).is_err());
    // data chunk claims 4 GiB: lenient read of what's there.
    let mut wav = riff(&[(b"fmt ", fmt(1, 2, 44_100, 16)), (b"data", vec![0; 16])]);
    let d = wav.windows(4).position(|w| w == b"data").unwrap();
    wav[d + 4..d + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    let (info, buf) = decode(&wav, None).unwrap();
    assert_eq!((info.frames, buf.frames()), (4, 4));
    // An unknown chunk with an absurd size before fmt.
    let mut wav = riff(&[(b"junk", vec![0; 4]), (b"fmt ", fmt(1, 2, 44_100, 16)), (b"data", vec![0; 16])]);
    wav[16..20].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    assert!(decode(&wav, None).is_err());
    // RF64 with a ds64 claiming an exabyte of data.
    let mut ds64 = Vec::new();
    ds64.extend_from_slice(&u64::MAX.to_le_bytes());
    ds64.extend_from_slice(&u64::MAX.to_le_bytes());
    ds64.extend_from_slice(&u64::MAX.to_le_bytes());
    ds64.extend_from_slice(&0u32.to_le_bytes());
    let mut rf = riff(&[(b"ds64", ds64), (b"fmt ", fmt(1, 1, 8_000, 8)), (b"data", vec![0x80; 10])]);
    rf[..4].copy_from_slice(b"RF64");
    let d = rf.windows(4).rposition(|w| w == b"data").unwrap();
    rf[d + 4..d + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    let (info, buf) = decode(&rf, None).unwrap();
    assert_eq!((info.frames, buf.frames()), (10, 10));
    // AIFF COMM claims more frames than present; SSND offset beyond the end.
    let mut aiff = small(FileFormat::Aiff, BitDepth::Int16, 1);
    let comm = aiff.windows(4).position(|w| w == b"COMM").unwrap();
    aiff[comm + 10..comm + 14].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(decode(&aiff, None).unwrap().1.frames(), 300);
    let ssnd = aiff.windows(4).position(|w| w == b"SSND").unwrap();
    aiff[ssnd + 8..ssnd + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(decode(&aiff, None).unwrap().1.frames(), 0);
}

fn caf(chunks: &[(&[u8; 4], i64, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"caff\0\x01\0\0".to_vec();
    for (kind, size, body) in chunks {
        out.extend_from_slice(*kind);
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(body);
    }
    out
}

fn caf_desc(format: &[u8; 4], bytes_per_packet: u32, frames_per_packet: u32, channels: u32, bits: u32) -> (&'static [u8; 4], i64, Vec<u8>) {
    let mut d = 48_000f64.to_be_bytes().to_vec();
    d.extend_from_slice(format);
    for v in [0, bytes_per_packet, frames_per_packet, channels, bits] {
        d.extend_from_slice(&v.to_be_bytes());
    }
    (b"desc", 32, d)
}

fn packet_table(packets: i64, frames: i64, entries: &[u8]) -> Vec<u8> {
    [&packets.to_be_bytes()[..], &frames.to_be_bytes(), &[0; 8], entries].concat()
}

#[test]
fn caf_sizes_beyond_the_file_are_errors_not_aborts() {
    let aac = || caf_desc(b"aac ", 0, 1024, 2, 0);
    let huge = 1i64 << 50;
    let cookie = caf(&[aac(), (b"kuki", huge, vec![0; 8])]);
    let files = [
        cookie.clone(),
        [vec![0; 100], cookie].concat(),
        caf(&[aac(), (b"data", -1, [&[0; 4][..], b"kuki", &huge.to_be_bytes()].concat())]),
        caf(&[aac(), (b"pakt", 24, packet_table(huge, 1024, &[])), (b"data", -1, vec![0; 16])]),
        caf(&[aac(), (b"pakt", 32, packet_table(1, 1024, &[0x82, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0])), (b"data", -1, vec![0; 64])]),
        caf(&[aac(), (b"pakt", 25, packet_table(1, 1024, &[0x80]))]),
    ];
    for f in files {
        assert!(matches!(probe(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
        assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
    }
    let constant = caf(&[caf_desc(b"lpcm", 2, 1, 1, 16), (b"pakt", 25, packet_table(1, 1, &[1])), (b"data", -1, vec![0; 16])]);
    assert!(matches!(decode(&constant, None), Err(AudioError::Malformed(m)) if m.contains("lists packets")));
}

#[test]
fn well_formed_caf_still_reads() {
    let pcm: Vec<u8> = (0..480i16).flat_map(|i| (i * 50).to_be_bytes()).collect();
    let chan = [((100u32 << 16) | 1).to_be_bytes(), [0; 4], [0; 4]].concat();
    let data = [vec![0; 4], pcm].concat();
    let lpcm = caf(&[caf_desc(b"lpcm", 2, 1, 1, 16), (b"chan", 12, chan), (b"free", 8, vec![0; 8]), (b"data", data.len() as i64, data)]);
    let (info, buf) = decode(&lpcm, None).unwrap();
    assert_eq!((info.format, buf.frames()), (FileFormat::Caf, 480));
    let table = packet_table(3, 3072, &[10, 0x81, 0x48, 5]);
    let aac = caf(&[caf_desc(b"aac ", 0, 1024, 2, 0), (b"pakt", table.len() as i64, table), (b"data", 219, vec![0; 219])]);
    assert_eq!(probe(&aac, None).unwrap().frames, 3072);
}

fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [&(8 + body.len() as u32).to_be_bytes()[..], kind, body].concat()
}

/// A mono 16-bit, 48 kHz audio sample entry of `version`, its version fields and codec atoms.
fn sample_entry(kind: &[u8; 4], version: u8, fields: &[u8], codec: &[u8]) -> Vec<u8> {
    let base = [0, 0, 0, 0, 0, 0, 0, 1, 0, version, 0, 0, 0, 0, 0, 0, 0, 1, 0, 16, 0, 0, 0, 0];
    atom(kind, &[&base[..], &(48_000u32 << 16).to_be_bytes(), fields, codec].concat())
}

/// An MP4 of `frames` 2-byte samples described by `entry`; `swap` may replace any version-0 atom.
fn mp4(frames: u32, entry: &[u8], swap: impl Fn(&[u8; 4], Vec<u8>) -> Vec<u8>) -> Vec<u8> {
    let full = |kind: &[u8; 4], fields: &[u32]| {
        let body: Vec<u8> = [0].iter().chain(fields).flat_map(|v: &u32| v.to_be_bytes()).collect();
        swap(kind, atom(kind, &body))
    };
    let moov = |offset: u32| {
        let stsd = atom(b"stsd", &[&[0, 0, 0, 0, 0, 0, 0, 1][..], entry].concat());
        let stbl =
            [stsd, full(b"stts", &[1, frames, 1]), full(b"stsc", &[1, 1, frames, 1]), full(b"stsz", &[2, frames]), full(b"stco", &[1, offset])];
        let minf = [full(b"smhd", &[0]), atom(b"stbl", &stbl.concat())].concat();
        let mdia = [full(b"mdhd", &[0, 0, 48_000, frames, 0]), full(b"hdlr", &[0, u32::from_be_bytes(*b"soun"), 0, 0, 0, 0]), atom(b"minf", &minf)];
        let trak = [full(b"tkhd", &[0; 20]), atom(b"mdia", &mdia.concat())].concat();
        atom(b"moov", &[full(b"mvhd", &[[0, 0, 48_000, frames, 1 << 16].as_slice(), &[0; 18], &[2]].concat()), atom(b"trak", &trak)].concat())
    };
    let ftyp = atom(b"ftyp", b"M4A \0\0\0\0M4A isom");
    let offset = (ftyp.len() + moov(0).len() + 8) as u32;
    let pcm: Vec<u8> = (0..frames as i16).flat_map(|i| (i * 40).to_le_bytes()).collect();
    [ftyp, moov(offset), atom(b"mdat", &pcm)].concat()
}

#[test]
fn mp4_sizes_beyond_the_file_are_errors_not_aborts() {
    let pcm = sample_entry(b"sowt", 0, &[], &[]);
    let (_, buf) = decode(&mp4(480, &pcm, |_, a| a), None).unwrap();
    assert_eq!(buf.frames(), 480);
    let huge = |kind: &[u8; 4], a: Vec<u8>| [&1u32.to_be_bytes()[..], kind, &(1u64 << 60).to_be_bytes(), &a[8..]].concat();
    // The first field after version and flags: stts's entry count, stsz's sample size.
    let first_field_max = |mut a: Vec<u8>| {
        a[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
        a
    };
    let sizes: Vec<u8> = (0..480u32).flat_map(|i| if i == 479 { u32::MAX } else { 2 }.to_be_bytes()).collect();
    let per_sample = atom(b"stsz", &[&[0; 8][..], &480u32.to_be_bytes(), &sizes].concat());
    // A header that starts inside its container is read whole, past the container's end.
    let mdia = atom(b"mdia", &1u32.to_be_bytes());
    let straddling = [&b"hdlr"[..], &(1u64 << 60).to_be_bytes(), &[0; 24]].concat();
    let files = [
        mp4(480, &pcm, |k, a| if k == b"hdlr" { huge(k, a) } else { a }),
        mp4(480, &pcm, |k, a| if k == b"stts" { first_field_max(a) } else { a }),
        mp4(480, &pcm, |k, a| if k == b"stsz" { first_field_max(a) } else { a }),
        mp4(480, &pcm, |k, a| if k == b"stsz" { per_sample.clone() } else { a }),
        [atom(b"ftyp", b"M4A \0\0\0\0M4A isom"), atom(b"moov", &atom(b"trak", &mdia)), straddling].concat(),
    ];
    for f in files {
        assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
    }
}

#[test]
fn atoms_running_past_their_parent_are_walked_once() {
    // Each moov holds a trak that runs to the end of the file; symphonia stops after the first.
    let unit = atom(b"moov", &[&u32::MAX.to_be_bytes()[..], b"trak"].concat());
    let f = [atom(b"ftyp", b"M4A \0\0\0\0M4A isom"), unit.repeat(1 << 18)].concat();
    assert!(decode(&f, None).is_err());
}

#[test]
fn packets_longer_than_any_codec_uses_are_refused() {
    let be = |v: &[u32]| v.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<u8>>();
    let v2 = |frames: u32| [&be(&[72])[..], &48_000f64.to_be_bytes(), &be(&[1, 0x7f00_0000, 16, 4, 2, frames])].concat();
    let lpcm = |frames: u32| sample_entry(b"lpcm", 2, &v2(frames), &[]);
    let config = |frames: u32| [&be(&[0, frames])[..], &[0, 16, 40, 10, 14, 1, 0, 255], &be(&[0, 0, 48_000])].concat();
    let alac = |frames: u32| sample_entry(b"alac", 0, &[], &atom(b"alac", &config(frames)));
    let (_, buf) = decode(&mp4(480, &lpcm(1), |_, a| a), None).unwrap();
    assert_eq!(buf.frames(), 480);
    for entry in [lpcm(1 << 21), alac(1 << 21)] {
        assert!(matches!(decode(&mp4(480, &entry, |_, a| a), None), Err(AudioError::Malformed(m)) if m.contains("declares packets")));
    }
}

#[test]
fn flac_metadata_fields_beyond_their_block_are_refused() {
    let with_block = |kind: u8, body: &[u8]| {
        let f = small(FileFormat::Flac, BitDepth::Int16, 1);
        let head = ((0x80 | u32::from(kind)) << 24 | body.len() as u32).to_be_bytes();
        [&f[..4], &[f[4] & 0x7f], &f[5..42], &head, body, &f[42..]].concat()
    };
    let picture = |data: u32| [&[0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0][..], &[0; 16], &data.to_be_bytes(), &[1, 2, 3]].concat();
    assert_eq!(decode(&with_block(6, &picture(3)), None).unwrap().1.frames(), 300);
    let comment = [&0u32.to_le_bytes()[..], &1u32.to_le_bytes(), &u32::MAX.to_le_bytes()].concat();
    for f in [with_block(6, &picture(0x7fff_fff0)), with_block(4, &comment)] {
        assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than its block")));
    }
}

#[test]
fn an_id3_tag_before_the_audio_is_skipped_unread() {
    let frame = [&b"TIT2"[..], &0x7fff_ffffu32.to_be_bytes(), &[0, 0, 3, b'h', b'i', 0]].concat();
    let tag = [&b"ID3\x03\0\0\0\0\0"[..], &[frame.len() as u8], &frame].concat();
    let flac = small(FileFormat::Flac, BitDepth::Int16, 1);
    assert_eq!(decode(&[tag, flac].concat(), None).unwrap().1.frames(), 300);
}

/// Container magic inside an MP3 frame payload must not change detection or make probing reject the stream.
#[test]
fn a_valid_mp3_with_incidental_container_magic_still_probes() {
    let mut frame = vec![0u8; 417];
    frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // MPEG-1 Layer III, 128 kbps, 44.1 kHz, stereo
    let mut f: Vec<u8> = (0..8).flat_map(|_| frame.clone()).collect();
    let at = 417 * 2 + 10; // Inside frame 2's payload, leaving every frame header intact.
    f[at..at + 8].copy_from_slice(b"caff\0\x01\0\0");
    f[at + 8..at + 12].copy_from_slice(b"kuki");
    f[at + 12..at + 20].copy_from_slice(&(1u64 << 50).to_be_bytes());
    assert_eq!(detect_format(&f, None), FileFormat::Mp3);
    let info = probe(&f, None).expect("valid MP3 with incidental container magic");
    assert_eq!((info.format, info.sample_rate, info.channels), (FileFormat::Mp3, 44_100, 2));
}

/// Symphonia 0.6's CAF/FLAC/MP4 `Scoreable` implementations are unconditional
/// (`Supported(255)`), so a container magic before the first audio frame *is* the reader the probe
/// selects; there is no API to ask the probe to skip it. The preflight must therefore keep
/// refusing an oversized CAF cookie instead of ignoring the marker and playing the MPEG frames
/// behind it (which would also let a real malformed container bypass the size guard).
#[test]
fn a_container_marker_before_the_first_audio_frame_is_the_probed_candidate() {
    let mut frame = vec![0u8; 417];
    frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // MPEG-1 Layer III, 128 kbps, 44.1 kHz, stereo
    let mp3: Vec<u8> = (0..8).flat_map(|_| frame.clone()).collect();
    let malformed = caf(&[caf_desc(b"aac ", 0, 1024, 2, 0), (b"kuki", 1i64 << 50, vec![0; 8])]);
    for f in [[vec![0u8], malformed.clone(), mp3.clone()].concat(), [vec![0u8; 32], malformed.clone(), mp3.clone()].concat()] {
        // The hint still names the raw stream, but the probe sees the CAF marker first.
        assert_eq!(detect_format(&f, Some("mp3")), FileFormat::Mp3);
        assert!(matches!(probe(&f, Some("mp3")), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
        assert!(matches!(decode(&f, Some("mp3")), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
    }
    // Plain leading junk without format magic is tolerated, as before.
    let f = [vec![0u8; 32], mp3].concat();
    assert_eq!(detect_format(&f, Some("mp3")), FileFormat::Mp3);
    let info = probe(&f, Some("mp3")).expect("junk before a valid MP3");
    assert_eq!((info.format, info.sample_rate, info.channels), (FileFormat::Mp3, 44_100, 2));
}

/// More false `0xFFEx` pairs than the preflight's scoring budget must not make a later container
/// magic inside the valid audio payload authoritative. Past the cap the scan is unchecked and the
/// probe selects the MPEG reader at the first real frame.
#[test]
fn an_exhausted_audio_score_budget_does_not_reject_the_mp3_behind_it() {
    let mut frame = vec![0u8; 417];
    frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
    let mut audio: Vec<u8> = (0..8).flat_map(|_| frame.clone()).collect();
    let at = 417 * 2 + 10; // Inside frame 2's payload, leaving every frame header intact.
    audio[at..at + 8].copy_from_slice(b"caff\0\x01\0\0");
    audio[at + 8..at + 12].copy_from_slice(b"kuki");
    audio[at + 12..at + 20].copy_from_slice(&(1u64 << 50).to_be_bytes());
    let false_syncs: Vec<u8> = (0..256).flat_map(|_| [0xFFu8, 0xE2]).collect();
    let f = [false_syncs, audio].concat();
    assert_eq!(detect_format(&f, Some("mp3")), FileFormat::Mp3);
    let info = probe(&f, Some("mp3")).expect("valid MP3 after an exhausted audio score budget");
    assert_eq!((info.format, info.sample_rate, info.channels), (FileFormat::Mp3, 44_100, 2));
    let (decoded, buf) = decode(&f, Some("mp3")).expect("decode the same MP3");
    assert_eq!((decoded.format, decoded.sample_rate, decoded.channels), (FileFormat::Mp3, 44_100, 2));
    assert_eq!(decoded.frames, buf.frames() as u64);
}

/// Past the score budget the preflight does not run, so if the probe then selects a container
/// whose reader allocates from sizes in the file, the post-probe refusal still applies.
#[test]
fn a_container_selected_without_its_size_check_is_refused() {
    let flac = small(FileFormat::Flac, BitDepth::Int16, 1);
    // Exactly the cap of false candidates, then one more that the reader rejects (invalid
    // bit-rate), so the preflight stops unchecked before reaching the FLAC marker behind it.
    let mut false_syncs: Vec<u8> = (0..256).flat_map(|_| [0xFFu8, 0xE2]).collect();
    false_syncs.extend_from_slice(&[0xFF, 0xFB, 0xF0, 0x00]);
    let f = [false_syncs, flac].concat();
    assert_eq!(detect_format(&f, None), FileFormat::Mp3);
    assert!(matches!(probe(&f, None), Err(AudioError::Malformed(m)) if m.contains("was not size-checked")));
    assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("was not size-checked")));
}

/// A zero-frame FLAC after arbitrary bytes or ID3 tags is still detected and reported as an empty stream, not a
/// decode failure.
#[test]
fn zero_frame_flac_after_a_prefix_is_still_empty() {
    let flac =
        encode(&AudioBuffer::new(48_000, 1, 0), &EncodeOptions { format: FileFormat::Flac, bit_depth: BitDepth::Int16, dither: false, bwf: None })
            .unwrap();
    let tag = id3_tag(b"tag");
    for (f, detected) in [
        ([vec![0x7A; 64], flac.clone()].concat(), FileFormat::Other),
        ([tag.clone(), vec![0x7A; 16], flac.clone()].concat(), FileFormat::Mp3),
        ([tag.clone(), tag, flac.clone()].concat(), FileFormat::Mp3),
    ] {
        assert_eq!(detect_format(&f, None), detected);
        let info = probe(&f, None).expect("empty FLAC after a prefix");
        assert_eq!((info.format, info.sample_rate, info.channels, info.frames), (FileFormat::Flac, 48_000, 1, 0));
        assert_eq!(decode(&f, None).unwrap().1.frames(), 0);
    }
}

/// Build one MPEG-4 AAC-LC ADTS frame at 48 kHz stereo around `payload`, with the 13-bit length covering the 7-byte
/// header.
fn adts_frame(payload: &[u8]) -> Vec<u8> {
    // MPEG-4 AAC-LC, 48 kHz, stereo; the 13-bit frame length includes this 7-byte header.
    let len = 7 + payload.len();
    let mut h = [0xFF, 0xF1, 0x4C, 0x00, 0x00, 0x1F, 0xFC];
    h[3] = 0x80 | ((len >> 11) & 0x03) as u8; // stereo channel config low bits
    h[4] = ((len >> 3) & 0xFF) as u8;
    h[5] = (((len & 0x07) as u8) << 5) | 0x1F;
    [&h[..], payload].concat()
}

/// False MPEG sync prefixes must not stop a following zero-frame FLAC from probing as empty.
#[test]
fn zero_frame_flac_after_a_false_mpeg_sync_is_still_empty() {
    let flac =
        encode(&AudioBuffer::new(48_000, 1, 0), &EncodeOptions { format: FileFormat::Flac, bit_depth: BitDepth::Int16, dither: false, bwf: None })
            .unwrap();
    for junk in [vec![0xFF, 0xE0, 0x00], vec![0xFF, 0xF0, 0x00], vec![0xFF, 0xFB, 0xF0, 0x00]] {
        let f = [junk, flac.clone()].concat();
        let info = probe(&f, None).expect("empty FLAC after a false sync");
        assert_eq!((info.format, info.sample_rate, info.channels, info.frames), (FileFormat::Flac, 48_000, 1, 0));
        assert_eq!(decode(&f, None).unwrap().1.frames(), 0);
    }
}

/// A false MPEG sync prefix must not let malformed CAF or MP4 size fields bypass the container preflight.
#[test]
fn a_false_sync_does_not_bypass_container_checks() {
    let bad_caf = caf(&[
        caf_desc(b"aac ", 0, 1024, 2, 0),
        (b"pakt", 32, packet_table(1, 1024, &[0x82, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0])),
        (b"data", -1, vec![0; 64]),
    ]);
    assert!(matches!(probe(&bad_caf, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
    let f = [vec![0xFF, 0xE0, 0x00], bad_caf].concat();
    assert!(matches!(probe(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
    assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));

    let pcm = sample_entry(b"sowt", 0, &[], &[]);
    let bad_mp4 = mp4(480, &pcm, |kind, mut a| {
        if kind == b"stsz" {
            a[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
        }
        a
    });
    assert!(probe(&bad_mp4, None).is_err());
    let f = [vec![0xFF, 0xE0, 0x00], bad_mp4].concat();
    assert!(matches!(probe(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
    assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
}

/// Container magic inside a valid ADTS payload must not override AAC detection or make probing fail.
#[test]
fn a_valid_adts_stream_with_incidental_container_magic_still_probes() {
    let payload = [0u8; 400];
    let mut frames: Vec<Vec<u8>> = (0..8).map(|_| adts_frame(&payload)).collect();
    let at = 100; // Inside frame 2's payload, leaving every frame header intact.
    frames[2][7 + at..7 + at + 12].copy_from_slice(b"caff\0\x01\0\0kuki");
    let f: Vec<u8> = frames.concat();
    assert_eq!(detect_format(&f, None), FileFormat::Aac);
    let info = probe(&f, None).expect("valid ADTS with incidental container magic");
    assert_eq!((info.format, info.sample_rate, info.channels), (FileFormat::Aac, 48_000, 2));
}

/// A later valid CAF must not excuse an earlier malformed container that the preflight or probe rejects.
#[test]
fn an_earlier_malformed_container_is_not_excused_by_a_later_valid_one() {
    let malformed = caf(&[caf_desc(b"aac ", 0, 1024, 2, 0), (b"kuki", 1i64 << 50, vec![0; 8])]);
    let pcm: Vec<u8> = (0..32i16).flat_map(|i| (i * 50).to_be_bytes()).collect();
    let data = [vec![0; 4], pcm].concat();
    let valid = caf(&[caf_desc(b"lpcm", 2, 1, 1, 16), (b"data", data.len() as i64, data)]);
    let f = [vec![0u8; 32], malformed, valid].concat();
    assert!(matches!(probe(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
    assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
}

/// Truncated ID3 bodies, declared tag sizes beyond the file, and a complete tag with no audio after it are malformed
/// for both probe and decode.
#[test]
fn truncated_or_oversized_id3_tags_are_malformed() {
    let cases = vec![[&id3_header(100)[..], b"xx"].concat(), id3_header(0x0FFF_FFFF).to_vec(), id3_tag(b"tag")];
    for f in cases {
        assert!(matches!(probe(&f, None), Err(AudioError::Malformed(_))));
        assert!(matches!(decode(&f, None), Err(AudioError::Malformed(_))));
    }
}

#[test]
fn wav_info_entries_beyond_the_file_are_refused() {
    let alaw = |len: u32| {
        let info = [&b"INFO"[..], b"INAM", &len.to_le_bytes(), b"hi"].concat();
        riff(&[(b"fmt ", [fmt(6, 1, 8_000, 8), vec![0, 0]].concat()), (b"LIST", info), (b"data", vec![0xD5; 800])])
    };
    assert_eq!(decode(&alaw(2), None).unwrap().1.frames(), 800);
    let mut f = alaw(0x7fff_fff0);
    f[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    let list = f.windows(4).position(|w| w == b"LIST").unwrap();
    f[list + 4..list + 8].copy_from_slice(&0x7fff_fffcu32.to_le_bytes());
    assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
}

#[test]
fn a_wave_behind_another_riff_form_is_checked() {
    let info = [&b"INFO"[..], b"INAM", &0x7fff_fff0u32.to_le_bytes(), b"hi"].concat();
    let mut wav = riff(&[(b"fmt ", [fmt(6, 1, 8_000, 8), vec![0, 0]].concat()), (b"LIST", info), (b"data", vec![0xD5; 800])]);
    wav[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    let list = wav.windows(4).position(|w| w == b"LIST").unwrap();
    wav[list + 4..list + 8].copy_from_slice(&0x7fff_fffcu32.to_le_bytes());
    // The probe skips the AVI form and reads the WAVE behind it.
    let f = [&b"RIFF"[..], &4u32.to_le_bytes(), b"AVI ", &wav].concat();
    assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
}

#[test]
fn wav_info_entries_are_found_where_symphonia_reads_them() {
    let list = [&b"LIST"[..], &0x7fff_fffcu32.to_le_bytes(), b"INFO", b"INAM", &0x7fff_fff0u32.to_le_bytes(), b"hi"].concat();
    let alaw = |extra: u16| (b"fmt ", [fmt(6, 1, 8_000, 8), extra.to_le_bytes().to_vec()].concat());
    // A-law's extra bytes are read past its chunk; an odd list is padded twice.
    let files =
        [riff(&[alaw(12), (b"JUNK", [&[0; 4][..], &list].concat())]), [riff(&[alaw(0), (b"LIST", b"adtl\0".to_vec())]), vec![0], list].concat()];
    for mut f in files {
        f[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(decode(&f, None), Err(AudioError::Malformed(m)) if m.contains("larger than the file")));
    }
}

#[test]
fn many_tiny_chunks_terminate() {
    let mut chunks: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"fmt ", fmt(1, 1, 8_000, 16))];
    for _ in 0..200_000 {
        chunks.push((b"pad ", Vec::new()));
    }
    let wav = riff(&chunks);
    assert!(decode(&wav, None).is_err()); // no data chunk within the scan limit
}

#[test]
fn hand_built_wav_variants() {
    // 8-bit unsigned.
    let wav = riff(&[(b"fmt ", fmt(1, 1, 8_000, 8)), (b"data", vec![0x80, 0xFF, 0x00])]);
    let (info, buf) = decode(&wav, None).unwrap();
    assert_eq!(info.sample_format, SampleFormat::Int8);
    assert_eq!(buf.channels[0], vec![0.0, 127.0 / 128.0, -1.0]);
    // 64-bit float, with NaN replaced by silence.
    let mut data = Vec::new();
    for v in [0.25f64, -0.5, f64::NAN] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    let wav = riff(&[(b"fmt ", fmt(3, 1, 8_000, 64)), (b"data", data)]);
    let (info, buf) = decode(&wav, None).unwrap();
    assert_eq!(info.sample_format, SampleFormat::Float64);
    assert_eq!(buf.channels[0], vec![0.25, -0.5, 0.0]);
    // Unsupported PCM width.
    let wav = riff(&[(b"fmt ", fmt(3, 1, 8_000, 16)), (b"data", vec![0; 4])]);
    assert!(matches!(decode(&wav, None), Err(AudioError::Unsupported(_))));
    // No data chunk / no fmt chunk.
    assert!(decode(&riff(&[(b"fmt ", fmt(1, 1, 8_000, 16))]), None).is_err());
    assert!(decode(&riff(&[(b"data", vec![0; 4])]), None).is_err());
    // Short fmt chunk.
    assert!(decode(&riff(&[(b"fmt ", vec![1, 0, 1]), (b"data", vec![0; 4])]), None).is_err());
    // Unknown codec tag goes to symphonia and fails cleanly.
    let wav = riff(&[(b"fmt ", fmt(0x1234, 1, 8_000, 16)), (b"data", vec![0; 4])]);
    assert!(decode(&wav, None).is_err());
}

#[test]
fn aiff_sowt_little_endian() {
    let be = small(FileFormat::Aiff, BitDepth::Int16, 2);
    let (_, expect) = decode(&be, None).unwrap();
    // Rebuild as AIFC/sowt by hand.
    let comm = be.windows(4).position(|w| w == b"COMM").unwrap();
    let ssnd = be.windows(4).position(|w| w == b"SSND").unwrap();
    let mut comm_body = be[comm + 8..comm + 26].to_vec();
    comm_body.extend_from_slice(b"sowt");
    comm_body.extend_from_slice(&[0, 0]); // empty pstring, padded
    let mut ssnd_body = be[ssnd + 8..].to_vec();
    for pair in ssnd_body[8..].as_chunks_mut::<2>().0 {
        pair.swap(0, 1);
    }
    let mut body = b"AIFC".to_vec();
    for (id, data) in [(b"COMM", &comm_body), (b"SSND", &ssnd_body)] {
        body.extend_from_slice(id);
        body.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
    }
    let mut f = b"FORM".to_vec();
    f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    f.extend_from_slice(&body);
    let (info, got) = decode(&f, None).unwrap();
    assert_eq!(info.sample_format, SampleFormat::Int16);
    assert_eq!(got, expect);
}

#[test]
fn detect_format_by_magic_and_hint() {
    assert_eq!(detect_format(&small(FileFormat::Wav, BitDepth::Int16, 1), Some("mp3")), FileFormat::Wav);
    assert_eq!(detect_format(&small(FileFormat::Aiff, BitDepth::Int16, 1), None), FileFormat::Aiff);
    assert_eq!(detect_format(&small(FileFormat::Flac, BitDepth::Int16, 1), None), FileFormat::Flac);
    assert_eq!(detect_format(b"OggS\0\x02rest", None), FileFormat::Ogg);
    assert_eq!(detect_format(b"caff\0\x01\0\0", None), FileFormat::Caf);
    assert_eq!(detect_format(b"ID3\x04\0\0\0\0\0\x00", None), FileFormat::Mp3);
    let mut id3_flac = b"ID3\x04\0\0\0\0\0\x02xx".to_vec();
    id3_flac.extend_from_slice(b"fLaC");
    assert_eq!(detect_format(&id3_flac, None), FileFormat::Flac);
    assert_eq!(detect_format(&[0xFF, 0xFB, 0x90, 0x00], None), FileFormat::Mp3);
    assert_eq!(detect_format(&[0xFF, 0xF1, 0x50, 0x80], None), FileFormat::Aac);
    let mut m4a = b"\0\0\0\x20ftypM4A ".to_vec();
    m4a.extend_from_slice(b"....stsd\0\0\0\0\0\0\0\x01\0\0\0\x24alac");
    assert_eq!(detect_format(&m4a, None), FileFormat::Alac);
    assert_eq!(detect_format(b"\0\0\0\x20ftypM4A ", None), FileFormat::Aac);
    assert_eq!(detect_format(b"", Some(".WAV")), FileFormat::Wav);
    assert_eq!(detect_format(b"zz", Some("aif")), FileFormat::Aiff);
    assert_eq!(detect_format(b"zz", Some("m4a")), FileFormat::Aac);
    assert_eq!(detect_format(b"zz", Some("M4B")), FileFormat::Aac);
    assert_eq!(detect_format(b"zz", Some("flac")), FileFormat::Flac);
    assert_eq!(detect_format(b"zz", Some("xyz")), FileFormat::Other);
    assert_eq!(detect_format(b"zz", None), FileFormat::Other);
    assert_eq!(detect_format(b"", None), FileFormat::Other);
}

#[test]
fn garbage_in_compressed_containers_errors() {
    let garbage: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    for hint in [None, Some("mp3"), Some("ogg"), Some("flac"), Some("m4a"), Some("caf"), Some("aac"), Some("mkv"), Some("wav"), Some("aiff")] {
        assert!(decode(&garbage, hint).is_err(), "{hint:?}");
        assert!(probe(&garbage, hint).is_err(), "{hint:?}");
    }
    for magic in [&b"fLaC"[..], b"OggS", b"caff", b"ID3\x03\0\0\0\0\0\0", &[0xFF, 0xFB, 0x90, 0x00], b"\0\0\0\x20ftypM4A "] {
        let mut f = magic.to_vec();
        f.extend_from_slice(&garbage);
        let _ = probe(&f, None);
        assert!(decode(&f, None).map(|(_, b)| b.frames()).unwrap_or(0) < 1_000_000);
    }
    assert!(decode(&[], None).is_err());
}

/// A marker that passes its container check must not let the probe select a different container
/// without its own checks: the reader is refused when the probed format disagrees.
#[test]
fn a_probed_container_that_disagrees_with_the_checked_marker_is_refused() {
    // The junk byte hides both magics, so the hint picks the marker: the FLAC is checked, but the
    // probe reads the WAVE before it.
    let bytes = [vec![0], small(FileFormat::Wav, BitDepth::Int24, 2), small(FileFormat::Flac, BitDepth::Int24, 2)].concat();
    match decode(&bytes, Some("flac")) {
        Err(AudioError::Malformed(message)) => assert!(message.contains("probed container"), "unexpected message: {message}"),
        Err(other) => panic!("unexpected error kind: {other:?}"),
        Ok(_) => panic!("the mismatched container must be refused"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048), hint in 0usize..6) {
        let hint = [None, Some("wav"), Some("aiff"), Some("flac"), Some("mp3"), Some("ogg")][hint];
        let _ = detect_format(&bytes, hint);
        let _ = probe(&bytes, hint);
        let _ = decode(&bytes, hint);
    }

    #[test]
    fn random_bodies_behind_valid_headers_never_panic(body in proptest::collection::vec(any::<u8>(), 0..1024), which in 0usize..4) {
        let mut f = match which {
            0 => b"RIFF\xff\xff\xff\xffWAVE".to_vec(),
            1 => b"RF64\xff\xff\xff\xffWAVE".to_vec(),
            2 => b"FORM\xff\xff\xff\xffAIFC".to_vec(),
            _ => b"FORM\xff\xff\xff\xffAIFF".to_vec(),
        };
        f.extend_from_slice(&body);
        let _ = probe(&f, None);
        let _ = decode(&f, None);
    }

    #[test]
    fn random_audio_roundtrips_through_flac(samples in proptest::collection::vec(-1.2f32..1.2, 1..3000), channels in 1usize..4) {
        let buf = AudioBuffer::from_interleaved(48_000, channels, &samples);
        prop_assume!(buf.frames() > 0);
        let opts = EncodeOptions { format: FileFormat::Flac, bit_depth: BitDepth::Int24, dither: false, bwf: None };
        let (_, flac) = decode(&encode(&buf, &opts).unwrap(), None).unwrap();
        let (_, wav) = decode(&encode(&buf, &EncodeOptions { format: FileFormat::Wav, ..opts }).unwrap(), None).unwrap();
        prop_assert_eq!(flac, wav);
    }
}

/// An extensible `fmt ` chunk: cbSize, valid bits, mask, then the 16-byte subtype GUID.
fn extensible_fmt(cb_size: u16, valid_bits: u16, guid: &[u8]) -> Vec<u8> {
    let mut f = fmt(0xFFFE, 1, 44_100, 16);
    f.extend_from_slice(&cb_size.to_le_bytes());
    f.extend_from_slice(&valid_bits.to_le_bytes());
    f.extend_from_slice(&4u32.to_le_bytes());
    f.extend_from_slice(guid);
    f
}

const PCM_GUID: [u8; 16] = [1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71];

#[test]
fn extensible_fmt_is_checked_in_full() {
    let data = vec![0u8; 8];
    let good = riff(&[(b"fmt ", extensible_fmt(22, 16, &PCM_GUID)), (b"data", data.clone())]);
    assert_eq!(probe(&good, None).unwrap().sample_format, SampleFormat::Int16);
    // Subtype GUID cut short after the format code.
    let cut = riff(&[(b"fmt ", extensible_fmt(22, 16, &PCM_GUID[..4])), (b"data", data.clone())]);
    assert!(matches!(probe(&cut, None), Err(AudioError::Malformed(_))));
    assert!(matches!(decode(&cut, None), Err(AudioError::Malformed(_))));
    // cbSize too small for the extension.
    let small_cb = riff(&[(b"fmt ", extensible_fmt(0, 16, &PCM_GUID)), (b"data", data.clone())]);
    assert!(matches!(probe(&small_cb, None), Err(AudioError::Malformed(_))));
    // More valid bits than the 16-bit container holds.
    let wide = riff(&[(b"fmt ", extensible_fmt(22, 24, &PCM_GUID)), (b"data", data.clone())]);
    assert!(matches!(probe(&wide, None), Err(AudioError::Malformed(_))));
    // PCM-like prefix but a foreign GUID tail is not read as native PCM.
    let mut foreign = PCM_GUID;
    foreign[15] ^= 0xFF;
    let foreign = riff(&[(b"fmt ", extensible_fmt(22, 16, &foreign)), (b"data", data)]);
    assert!(!matches!(probe(&foreign, None), Ok(i) if i.sample_format == SampleFormat::Int16));
    // The Ambisonic PCM subtype is PCM.
    let ambi = [1, 0, 0, 0, 0x21, 0x07, 0xD3, 0x11, 0x86, 0x44, 0xC8, 0xC1, 0xCA, 0, 0, 0];
    let ambi = riff(&[(b"fmt ", extensible_fmt(22, 16, &ambi)), (b"data", vec![0u8; 8])]);
    assert_eq!(probe(&ambi, None).unwrap().sample_format, SampleFormat::Int16);
}
