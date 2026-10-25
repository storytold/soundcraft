//! Decode path for everything that isn't native PCM WAV/AIFF, backed by `symphonia`.

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ADPCM_G722, CODEC_ID_ADPCM_G726, CODEC_ID_ADPCM_G726LE, CODEC_ID_ADPCM_IMA_QT, CODEC_ID_ADPCM_IMA_WAV, CODEC_ID_ADPCM_MS,
    CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_PCM_ALAW, CODEC_ID_PCM_F32BE, CODEC_ID_PCM_F32LE,
    CODEC_ID_PCM_F64BE, CODEC_ID_PCM_F64LE, CODEC_ID_PCM_MULAW, CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{AudioCodecId, AudioCodecParameters, AudioDecoderOptions, CODEC_ID_NULL_AUDIO};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions, ReadBytes};
use symphonia::core::meta::MetadataOptions;

use crate::pcm::{MAX_DECODE_SAMPLES, be_u32};
use crate::{AudioBuffer, AudioError, AudioInfo, FileFormat, Result, SampleFormat};

/// The most frames a packet may declare: decoders allocate a packet's frames up front, and ALAC
/// takes its frame length from the file (encoders use 4096).
const MAX_PACKET_FRAMES: u64 = 1 << 20;

/// Turn a symphonia error into the audio layer's error, keeping "unsupported" and "too large"
/// distinct from a malformed file.
fn map_err(e: SymError) -> AudioError {
    match e {
        SymError::Unsupported(m) => AudioError::Unsupported(m.to_string()),
        SymError::LimitError(m) => AudioError::TooLarge(m.to_string()),
        other => AudioError::Malformed(other.to_string()),
    }
}

/// Run `f`, turning a panic inside the third-party decoder into an error.
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(AudioError::Malformed("decoder failed on malformed input".into())))
}

/// What [`open`] found: a reader, or a stream that holds no audio frames and has no reader.
enum Opened {
    Reader(Box<dyn FormatReader>),
    Empty(AudioInfo),
}

/// Skip a leading ID3v2 tag before probing.
///
/// Tags aren't used, and symphonia allocates whatever an ID3v2 frame claims, so the tag is skipped
/// with [`ReadBytes::ignore_bytes`] (which allocates nothing) instead of being handed to the
/// metadata reader. Returns the stream position the container starts at.
fn skip_leading_id3(mss: &mut MediaSourceStream<'_>, bytes: &[u8]) -> Result<usize> {
    loop {
        let at = usize::try_from(mss.pos()).unwrap_or(usize::MAX);
        let head = bytes.get(at..).unwrap_or_default();
        if head.len() < 10 || head.get(..3) != Some(b"ID3") {
            return Ok(at);
        }
        let size = head.get(6..10).map(|size| size.iter().fold(0u64, |v, &x| (v << 7) | u64::from(x & 0x7f))).unwrap_or(0);
        mss.ignore_bytes(10 + size).map_err(|e| AudioError::Malformed(e.to_string()))?;
    }
}

/// How far symphonia's probe scans for a format marker before giving up.
const MAX_PROBE_DEPTH: usize = 1024 * 1024;

/// The container marker each detected format starts with, if its reader needs size checks first.
fn container_markers(detected: FileFormat) -> &'static [&'static [u8; 4]] {
    match detected {
        // RF64 and BW64 are the >4 GiB RIFF forms.
        FileFormat::Wav => &[b"RIFF", b"RF64", b"BW64"],
        FileFormat::Flac => &[b"fLaC"],
        FileFormat::Caf => &[b"caff"],
        // MP4/M4A put `ftyp` after the first atom's size field.
        FileFormat::Aac | FileFormat::Alac => &[b"ftyp"],
        // Unknown files may carry junk before the container; symphonia's probe scans for markers.
        FileFormat::Other => &[b"caff", b"ftyp", b"fLaC", b"RIFF"],
        FileFormat::Aiff | FileFormat::Mp3 | FileFormat::Ogg => &[],
    }
}

/// The probe short names a container marker stands for.
fn expected_probe_format(marker: &[u8; 4]) -> &'static [&'static str] {
    match marker {
        b"caff" => &["caf"],
        // The isomp4 reader reads the `ftyp` atom.
        b"ftyp" => &["isomp4"],
        b"fLaC" => &["flac"],
        b"RIFF" | b"RF64" | b"BW64" => &["wave"],
        _ => &[],
    }
}

/// Probe short names whose demuxer allocates from sizes in the file, so the matching container
/// check has to run before the stream is decoded.
const SIZE_CHECKED_PROBE_FORMATS: &[&str] = &["caf", "flac", "isomp4", "wave"];

/// Find the first container marker at or after `at`, within symphonia's probe depth. A RIFF form
/// other than WAVE is skipped, as the probe skips it and reads a WAVE behind it.
fn find_marker<'a>(bytes: &[u8], at: usize, markers: &'a [&'static [u8; 4]]) -> Option<(usize, &'a [u8; 4])> {
    let end = bytes.len().min(at.saturating_add(MAX_PROBE_DEPTH));
    for pos in at..end.saturating_sub(3) {
        let window = bytes.get(pos..pos + 4)?;
        if let Some(marker) = markers.iter().find(|marker| window == marker.as_slice()) {
            if matches!(*marker, b"RIFF" | b"RF64" | b"BW64") && bytes.get(pos + 8..pos + 12) != Some(&b"WAVE"[..]) {
                continue;
            }
            return Some((pos, *marker));
        }
    }
    None
}

/// Probe for a container, checking the container it finds before its reader allocates whatever the
/// file claims.
///
/// The check has to run on the container the probe actually selects, but the probe reports that
/// container only through the reader it returns. The check therefore runs on the first marker the
/// detected format allows, and the reader is refused if its format is not the checked one: a
/// misleading marker must not let a later container slip through unchecked.
fn open(bytes: &[u8], detected: FileFormat) -> Result<Opened> {
    let mut mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), MediaSourceStreamOptions::default());
    let at = skip_leading_id3(&mut mss, bytes)?;
    let mut checked: Option<&'static [&'static str]> = None;
    if let Some((found, marker)) = find_marker(bytes, at, container_markers(detected)) {
        match marker {
            b"caff" => {
                crate::caf::check(bytes, found)?;
                checked = Some(expected_probe_format(marker));
            }
            b"ftyp" => {
                crate::mp4::check(bytes, found.saturating_sub(4))?;
                checked = Some(expected_probe_format(marker));
            }
            b"fLaC" => {
                crate::flac::check_metadata(bytes, found)?;
                if let Some(info) = crate::flac::empty_stream_info(bytes, found) {
                    return Ok(Opened::Empty(info));
                }
                checked = Some(expected_probe_format(marker));
            }
            b"RIFF" | b"RF64" | b"BW64" => {
                crate::wav::check_info(bytes, found)?;
                checked = Some(expected_probe_format(marker));
            }
            _ => {}
        }
    }
    let reader = symphonia::default::get_probe().probe(&Hint::new(), mss, FormatOptions::default(), MetadataOptions::default()).map_err(map_err)?;
    let probed = reader.format_info().short_name;
    // Only containers whose readers allocate from sizes in the file need the check. A stream the
    // probe reads with another reader (MP3, Ogg, AIFF, ...) has no such size fields, so a marker
    // found in its audio data must not reject it.
    if let Some(names) = checked.filter(|_| SIZE_CHECKED_PROBE_FORMATS.contains(&probed)) {
        if !names.contains(&probed) {
            return Err(AudioError::Malformed(format!("the probed container ({probed}) is not the one whose size checks ran")));
        }
    } else if checked.is_none() && SIZE_CHECKED_PROBE_FORMATS.contains(&probed) {
        return Err(AudioError::Malformed(format!("the probed container ({probed}) was not size-checked")));
    }
    Ok(Opened::Reader(reader))
}

/// The container `detected` stands for: the detection wins when it knows the file, otherwise the
/// decoded codec names the family.
fn format_from_codec(codec: AudioCodecId, detected: FileFormat) -> FileFormat {
    if detected != FileFormat::Other {
        return detected;
    }
    match codec {
        c if c == CODEC_ID_FLAC => FileFormat::Flac,
        c if c == CODEC_ID_MP3 || c == CODEC_ID_MP2 || c == CODEC_ID_MP1 => FileFormat::Mp3,
        c if c == CODEC_ID_AAC => FileFormat::Aac,
        c if c == CODEC_ID_ALAC => FileFormat::Alac,
        c if c == CODEC_ID_VORBIS => FileFormat::Ogg,
        _ => FileFormat::Other,
    }
}

/// The stored sample format, using the codec and its declared bit depth. Lossy and unusual
/// codecs decode to `Compressed`.
fn sample_format_of(p: &AudioCodecParameters) -> SampleFormat {
    let c = p.codec;
    if c == CODEC_ID_PCM_F32LE || c == CODEC_ID_PCM_F32BE {
        return SampleFormat::Float32;
    }
    if c == CODEC_ID_PCM_F64LE || c == CODEC_ID_PCM_F64BE {
        return SampleFormat::Float64;
    }
    let lossy = [
        CODEC_ID_MP1,
        CODEC_ID_MP2,
        CODEC_ID_MP3,
        CODEC_ID_AAC,
        CODEC_ID_VORBIS,
        CODEC_ID_OPUS,
        CODEC_ID_ADPCM_MS,
        CODEC_ID_ADPCM_IMA_WAV,
        CODEC_ID_ADPCM_IMA_QT,
        CODEC_ID_ADPCM_G722,
        CODEC_ID_ADPCM_G726,
        CODEC_ID_ADPCM_G726LE,
        CODEC_ID_PCM_ALAW,
        CODEC_ID_PCM_MULAW,
    ];
    let lossless = !lossy.contains(&c);
    match (lossless, p.bits_per_sample) {
        (true, Some(1..=8)) => SampleFormat::Int8,
        (true, Some(9..=16)) => SampleFormat::Int16,
        (true, Some(17..=24)) => SampleFormat::Int24,
        (true, Some(25..=32)) => SampleFormat::Int32,
        _ => SampleFormat::Compressed,
    }
}

/// The first track with a decodable audio codec, with the stream information the UI shows.
fn info_of(reader: &dyn FormatReader, detected: FileFormat) -> Result<(u32, AudioInfo)> {
    let (track, p) = reader
        .tracks()
        .iter()
        .find_map(|t| match t.codec_params.as_ref().and_then(CodecParameters::audio) {
            Some(p) if p.codec != CODEC_ID_NULL_AUDIO => Some((t, p)),
            _ => None,
        })
        .ok_or_else(|| AudioError::Unsupported("no decodable audio track".into()))?;
    let sample_rate = p.sample_rate.filter(|&r| r > 0).ok_or_else(|| AudioError::Malformed("unknown sample rate".into()))?;
    let channels = p.channels.as_ref().map(|c| c.count()).unwrap_or(0);
    let channels = u16::try_from(channels).map_err(|_| AudioError::Unsupported("too many channels".into()))?;
    Ok((
        track.id,
        AudioInfo {
            format: format_from_codec(p.codec, detected),
            sample_format: sample_format_of(p),
            sample_rate,
            channels,
            frames: track.num_frames.unwrap_or(0),
            bwf: None,
        },
    ))
}

/// Decode the whole first audio track. When `count_only`, samples are counted but not stored.
fn run(bytes: &[u8], detected: FileFormat, count_only: bool) -> Result<(AudioInfo, AudioBuffer)> {
    let mut reader = match open(bytes, detected)? {
        Opened::Reader(reader) => reader,
        Opened::Empty(info) => {
            let buf = AudioBuffer::new(info.sample_rate, usize::from(info.channels), 0);
            return Ok((info, buf));
        }
    };
    let (track_id, mut info) = info_of(reader.as_ref(), detected)?;
    if let Some(n) = reader.tracks().iter().find(|t| t.id == track_id).and_then(|t| t.num_frames) {
        let total = n.saturating_mul(u64::from(info.channels.max(1)));
        if !count_only && total > MAX_DECODE_SAMPLES {
            return Err(AudioError::TooLarge(format!("{total} samples exceeds the decode limit")));
        }
    }
    let params = reader
        .tracks()
        .iter()
        .find(|t| t.id == track_id)
        .and_then(|t| t.codec_params.as_ref())
        .and_then(CodecParameters::audio)
        .cloned()
        .ok_or_else(|| AudioError::Malformed("track vanished".into()))?;
    let alac = params.extra_data.as_deref().filter(|_| params.codec == CODEC_ID_ALAC).and_then(|d| be_u32(d, 0));
    if params.max_frames_per_packet.into_iter().chain(alac.map(u64::from)).any(|n| n > MAX_PACKET_FRAMES) {
        return Err(AudioError::Malformed("the file declares packets far longer than any codec uses".into()));
    }
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(&params, &AudioDecoderOptions::default()).map_err(map_err)?;

    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<Vec<f32>> = Vec::new();
    let mut frames: u64 = 0;
    let mut errors = 0usize;
    loop {
        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                if frames == 0 {
                    return Err(map_err(e));
                }
                log::warn!("stopping decode after demux error: {e}");
                break;
            }
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(m)) => {
                errors += 1;
                log::debug!("skipping corrupt packet: {m}");
                if errors > 1000 && frames == 0 {
                    return Err(AudioError::Malformed(format!("no decodable packets: {m}")));
                }
                continue;
            }
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(map_err(e)),
        };
        let spec = decoded.spec();
        let ch = spec.channels().count();
        if ch == 0 {
            continue;
        }
        if planar.is_empty() {
            // The decoded stream is authoritative for the channel count.
            info.channels = u16::try_from(ch).map_err(|_| AudioError::Unsupported("too many channels".into()))?;
            planar = (0..ch).map(|_| Vec::new()).collect();
        }
        let n = decoded.frames() as u64;
        frames = frames.saturating_add(n);
        if frames.saturating_mul(ch as u64) > MAX_DECODE_SAMPLES {
            return Err(AudioError::TooLarge("decoded audio exceeds the 4 GiB limit".into()));
        }
        if count_only {
            continue;
        }
        decoded.copy_to_vecs_planar::<f32>(&mut scratch);
        for (dst, src) in planar.iter_mut().zip(&scratch) {
            dst.extend(src.iter().map(|&s| if s.is_finite() { s } else { 0.0 }));
        }
    }
    if info.channels == 0 {
        return Err(AudioError::Malformed("stream contains no audio".into()));
    }
    if planar.is_empty() {
        planar = (0..usize::from(info.channels)).map(|_| Vec::new()).collect();
    }
    info.frames = if count_only { frames } else { planar.iter().map(Vec::len).min().unwrap_or(0) as u64 };
    let sample_rate = info.sample_rate;
    Ok((info, AudioBuffer { sample_rate, channels: planar }))
}

pub(crate) fn probe(bytes: &[u8], detected: FileFormat) -> Result<AudioInfo> {
    guarded(|| {
        let reader = match open(bytes, detected)? {
            Opened::Reader(reader) => reader,
            Opened::Empty(info) => return Ok(info),
        };
        let (_, info) = info_of(reader.as_ref(), detected)?;
        if info.frames > 0 && info.channels > 0 {
            return Ok(info);
        }
        // Length not in the header (e.g. MP3 without a Xing frame): count by decoding.
        run(bytes, detected, true).map(|(i, _)| i)
    })
}

pub(crate) fn decode(bytes: &[u8], detected: FileFormat) -> Result<(AudioInfo, AudioBuffer)> {
    guarded(|| run(bytes, detected, false))
}
