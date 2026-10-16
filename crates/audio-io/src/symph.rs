//! Decode path for everything that isn't native PCM WAV/AIFF, backed by `symphonia`.

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{
    CODEC_TYPE_AAC, CODEC_TYPE_ADPCM_G722, CODEC_TYPE_ADPCM_G726, CODEC_TYPE_ADPCM_G726LE, CODEC_TYPE_ADPCM_IMA_QT, CODEC_TYPE_ADPCM_IMA_WAV,
    CODEC_TYPE_ADPCM_MS, CODEC_TYPE_ALAC, CODEC_TYPE_FLAC, CODEC_TYPE_MP1, CODEC_TYPE_MP2, CODEC_TYPE_MP3, CODEC_TYPE_NULL, CODEC_TYPE_OPUS,
    CODEC_TYPE_PCM_ALAW, CODEC_TYPE_PCM_F32BE, CODEC_TYPE_PCM_F32LE, CODEC_TYPE_PCM_F64BE, CODEC_TYPE_PCM_F64LE, CODEC_TYPE_PCM_MULAW,
    CODEC_TYPE_VORBIS, CodecParameters, CodecType, DecoderOptions,
};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions, ReadBytes};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Instantiate;

use crate::pcm::{MAX_DECODE_SAMPLES, be_u32};
use crate::{AudioBuffer, AudioError, AudioInfo, FileFormat, Result, SampleFormat};

/// The most frames a packet may declare: decoders allocate a packet's frames up front, and ALAC
/// takes its frame length from the file (encoders use 4096).
const MAX_PACKET_FRAMES: u64 = 1 << 20;

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

/// Symphonia's probe loop (`Probe::format`, whose hint 0.5 ignores), checking the container it
/// finds before its reader allocates whatever the file claims.
fn open(bytes: &[u8]) -> Result<Opened> {
    let mut mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), MediaSourceStreamOptions::default());
    let fmt_opts = FormatOptions { enable_gapless: true, ..FormatOptions::default() };
    loop {
        let found = symphonia::default::get_probe().next(&mut mss).map_err(map_err)?;
        let at = usize::try_from(mss.pos()).unwrap_or(usize::MAX);
        let head = bytes.get(at..).unwrap_or_default();
        match (found, head.get(..10)) {
            (Instantiate::Format(reader), _) => {
                match head.get(..4) {
                    Some(b"caff") => crate::caf::check(bytes, at)?,
                    Some(b"ftyp") => crate::mp4::check(bytes, at.saturating_sub(4))?,
                    Some(b"fLaC") => {
                        crate::flac::check_metadata(bytes, at)?;
                        if let Some(info) = crate::flac::empty_stream_info(bytes, at) {
                            return Ok(Opened::Empty(info));
                        }
                    }
                    Some(b"RIFF") => crate::wav::check_info(bytes, at)?,
                    _ => {}
                }
                return reader(mss, &fmt_opts).map(Opened::Reader).map_err(map_err);
            }
            // Tags aren't used, and symphonia allocates whatever an ID3v2 frame claims: skip it.
            (Instantiate::Metadata(_), Some([b'I', b'D', b'3', _, _, _, size @ ..])) => {
                let size = size.iter().fold(0u64, |v, &x| (v << 7) | u64::from(x & 0x7f));
                mss.ignore_bytes(10 + size).map_err(|e| AudioError::Malformed(e.to_string()))?;
            }
            (Instantiate::Metadata(tag), _) => {
                tag(&MetadataOptions::default()).read_all(&mut mss).map_err(map_err)?;
            }
        }
    }
}

fn format_from_codec(codec: CodecType, detected: FileFormat) -> FileFormat {
    if detected != FileFormat::Other {
        return detected;
    }
    match codec {
        c if c == CODEC_TYPE_FLAC => FileFormat::Flac,
        c if c == CODEC_TYPE_MP3 || c == CODEC_TYPE_MP2 || c == CODEC_TYPE_MP1 => FileFormat::Mp3,
        c if c == CODEC_TYPE_AAC => FileFormat::Aac,
        c if c == CODEC_TYPE_ALAC => FileFormat::Alac,
        c if c == CODEC_TYPE_VORBIS => FileFormat::Ogg,
        _ => FileFormat::Other,
    }
}

fn sample_format_of(p: &CodecParameters) -> SampleFormat {
    let c = p.codec;
    if c == CODEC_TYPE_PCM_F32LE || c == CODEC_TYPE_PCM_F32BE {
        return SampleFormat::Float32;
    }
    if c == CODEC_TYPE_PCM_F64LE || c == CODEC_TYPE_PCM_F64BE {
        return SampleFormat::Float64;
    }
    let lossy = [
        CODEC_TYPE_MP1,
        CODEC_TYPE_MP2,
        CODEC_TYPE_MP3,
        CODEC_TYPE_AAC,
        CODEC_TYPE_VORBIS,
        CODEC_TYPE_OPUS,
        CODEC_TYPE_ADPCM_MS,
        CODEC_TYPE_ADPCM_IMA_WAV,
        CODEC_TYPE_ADPCM_IMA_QT,
        CODEC_TYPE_ADPCM_G722,
        CODEC_TYPE_ADPCM_G726,
        CODEC_TYPE_ADPCM_G726LE,
        CODEC_TYPE_PCM_ALAW,
        CODEC_TYPE_PCM_MULAW,
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

fn info_of(reader: &dyn FormatReader, detected: FileFormat) -> Result<(u32, AudioInfo)> {
    let track = reader
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| AudioError::Unsupported("no decodable audio track".into()))?;
    let p = &track.codec_params;
    let sample_rate = p.sample_rate.filter(|&r| r > 0).ok_or_else(|| AudioError::Malformed("unknown sample rate".into()))?;
    let channels = p.channels.map(|c| c.count()).unwrap_or(0);
    let channels = u16::try_from(channels).map_err(|_| AudioError::Unsupported("too many channels".into()))?;
    Ok((
        track.id,
        AudioInfo {
            format: format_from_codec(p.codec, detected),
            sample_format: sample_format_of(p),
            sample_rate,
            channels,
            frames: p.n_frames.unwrap_or(0),
            bwf: None,
        },
    ))
}

/// Decode the whole first audio track. When `count_only`, samples are counted but not stored.
fn run(bytes: &[u8], detected: FileFormat, count_only: bool) -> Result<(AudioInfo, AudioBuffer)> {
    let mut reader = match open(bytes)? {
        Opened::Reader(reader) => reader,
        Opened::Empty(info) => {
            let buf = AudioBuffer::new(info.sample_rate, usize::from(info.channels), 0);
            return Ok((info, buf));
        }
    };
    let (track_id, mut info) = info_of(reader.as_ref(), detected)?;
    if let Some(n) = reader.tracks().iter().find(|t| t.id == track_id).and_then(|t| t.codec_params.n_frames) {
        let total = n.saturating_mul(u64::from(info.channels.max(1)));
        if !count_only && total > MAX_DECODE_SAMPLES {
            return Err(AudioError::TooLarge(format!("{total} samples exceeds the decode limit")));
        }
    }
    let params = reader
        .tracks()
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.codec_params.clone())
        .ok_or_else(|| AudioError::Malformed("track vanished".into()))?;
    let alac = params.extra_data.as_deref().filter(|_| params.codec == CODEC_TYPE_ALAC).and_then(|d| be_u32(d, 0));
    if params.max_frames_per_packet.into_iter().chain(alac.map(u64::from)).any(|n| n > MAX_PACKET_FRAMES) {
        return Err(AudioError::Malformed("the file declares packets far longer than any codec uses".into()));
    }
    let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(map_err)?;

    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut frames: u64 = 0;
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    let mut errors = 0usize;
    loop {
        let packet = match reader.next_packet() {
            Ok(p) => p,
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
        if packet.track_id() != track_id {
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
        let spec = *decoded.spec();
        let ch = spec.channels.count();
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
        let needed = decoded.capacity() as u64;
        let sb = match &mut sbuf {
            Some(sb) if sb.capacity() >= decoded.capacity() * ch => sb,
            _ => sbuf.insert(SampleBuffer::<f32>::new(needed, spec)),
        };
        sb.copy_interleaved_ref(decoded);
        for frame in sb.samples().chunks_exact(ch) {
            for (dst, &s) in planar.iter_mut().zip(frame) {
                dst.push(if s.is_finite() { s } else { 0.0 });
            }
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
        let reader = match open(bytes)? {
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
