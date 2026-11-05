//! Decode path for everything that isn't native PCM WAV/AIFF, backed by `symphonia`.

use std::io::{Cursor, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};

use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ADPCM_G722, CODEC_ID_ADPCM_G726, CODEC_ID_ADPCM_G726LE, CODEC_ID_ADPCM_IMA_QT, CODEC_ID_ADPCM_IMA_WAV, CODEC_ID_ADPCM_MS,
    CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_PCM_ALAW, CODEC_ID_PCM_F32BE, CODEC_ID_PCM_F32LE,
    CODEC_ID_PCM_F64BE, CODEC_ID_PCM_F64LE, CODEC_ID_PCM_MULAW, CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{AudioCodecId, AudioCodecParameters, AudioDecoderOptions, CODEC_ID_NULL_AUDIO};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::{Hint, ProbeOptions, ProbeableFormat, Score, Scoreable};
use symphonia::core::formats::{FormatOptions, FormatReader, Track};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions, ScopedStream};
use symphonia::core::meta::MetadataOptions;

use crate::pcm::{MAX_DECODE_SAMPLES, be_u32};
use crate::{AudioBuffer, AudioError, AudioInfo, FileFormat, Result, SampleFormat};

/// The most frames a packet may declare: decoders allocate a packet's frames up front, and ALAC
/// takes its frame length from the file (encoders use 4096).
const MAX_PACKET_FRAMES: u64 = 1 << 20;

/// How far [`first_marker`] looks for the first format marker: the same 1 MiB a default symphonia
/// probe scans.
const MAX_PREFIX_SCAN: usize = 1 << 20;

/// How many MPEG/ADTS candidates [`first_marker`] may hand to the readers' expensive `Scoreable`
/// implementations. A crafted 1 MiB prefix of repeated sync markers otherwise asks for hundreds
/// of thousands of scores, each allowed up to symphonia's 16 KiB score depth. Once the budget is
/// spent, the scan stops at the next audio candidate: a container-like byte string later in an
/// unscored audio payload must not become authoritative. A container marker reached first is still
/// checked; an unchecked one the probe selects is refused after probing.
const MAX_AUDIO_SCORE_ATTEMPTS: usize = 256;

/// A per-scan budget for `Scoreable::score` calls. Taking a slot consumes it even when the score
/// rejects the candidate, so a stream of false syncs cannot multiply the work.
struct ScoreBudget {
    used: usize,
}

impl ScoreBudget {
    /// Start a scan with no expensive score attempts spent.
    fn new() -> Self {
        Self { used: 0 }
    }

    /// Spend one score attempt, returning false once the per-scan budget is exhausted.
    fn take(&mut self) -> bool {
        if self.exhausted() {
            return false;
        }
        self.used = self.used.saturating_add(1);
        true
    }

    /// Whether no attempt is left for this scan.
    fn exhausted(&self) -> bool {
        self.used >= MAX_AUDIO_SCORE_ATTEMPTS
    }
}

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

/// Decoder options for every compressed format. Gapless playback used to be a
/// `FormatOptions` flag in symphonia 0.5; in 0.6 the decoder option (on by default) trims the
/// encoder delay and padding that the demuxer reports on each packet.
fn decoder_options() -> AudioDecoderOptions {
    AudioDecoderOptions::default().gapless(true)
}

/// The first byte after any ID3v2 tags at the start of `b`. Symphonia's tag reader allocates
/// whatever a frame claims, so the tags are skipped, not read: they are not used here.
fn skip_id3v2(b: &[u8]) -> usize {
    let mut at = 0usize;
    loop {
        match b.get(at..).and_then(crate::id3v2_len) {
            Some(n) if n > 0 => at = at.saturating_add(n),
            _ => return at,
        }
    }
}

/// The first format marker the probe can reach from `start`, within its 1 MiB scan limit.
#[derive(Clone, Copy)]
enum FirstMarker {
    Flac(usize),
    Caf(usize),
    Mp4(usize),
    Wav(usize),
    /// MPEG audio, Ogg, Matroska or AIFF: formats with no separate container size check.
    Audio,
}

/// What the preflight container checks covered before the probe built a reader.
enum Precheck {
    /// A zero-frame FLAC: returned without a reader, because Symphonia 0.6's reader needs a first
    /// audio frame to resynchronize to.
    Empty(AudioInfo),
    /// The size checks ran for the container the probe reports under this short name.
    Checked(&'static str),
    /// No container size check was needed.
    Unchecked,
}

/// Probe short names whose demuxer allocates from sizes in the file, so the matching container
/// check has to run before the stream is decoded.
const SIZE_CHECKED_PROBE_FORMATS: &[&str] = &["caf", "flac", "isomp4", "wave"];

/// The container markers to check for the family `detected` names. `None` is a raw audio stream
/// (MP3, Ogg, AIFF), whose payload may contain container-like bytes: the unrestricted scan below
/// finds a container only when one starts the stream, before any audio marker.
fn container_markers(detected: FileFormat) -> Option<&'static [&'static [u8; 4]]> {
    match detected {
        // RF64 and BW64 are the >4 GiB RIFF forms.
        FileFormat::Wav => Some(&[b"RIFF", b"RF64", b"BW64"]),
        FileFormat::Flac => Some(&[b"fLaC"]),
        FileFormat::Caf => Some(&[b"caff"]),
        // MP4/M4A put `ftyp` after the first atom's size field.
        FileFormat::Aac | FileFormat::Alac => Some(&[b"ftyp"]),
        // Unknown files may carry junk before the container; symphonia's probe scans for markers.
        FileFormat::Other => Some(&[b"caff", b"ftyp", b"fLaC", b"RIFF"]),
        FileFormat::Aiff | FileFormat::Mp3 | FileFormat::Ogg => None,
    }
}

/// The probe short name a container marker stands for.
fn expected_probe_format(marker: &[u8; 4]) -> &'static str {
    match marker {
        b"caff" => "caf",
        // The isomp4 reader reads the `ftyp` atom.
        b"ftyp" => "isomp4",
        b"fLaC" => "flac",
        // RIFF, RF64 and BW64.
        _ => "wave",
    }
}

/// Find the first marker of `markers` at or after `at`, within symphonia's probe depth. A RIFF
/// form other than WAVE is skipped, as the probe skips it and reads a WAVE behind it.
fn find_container_marker<'a>(bytes: &[u8], at: usize, markers: &'a [&'static [u8; 4]]) -> Option<(usize, &'a [u8; 4])> {
    let end = bytes.len().min(at.saturating_add(MAX_PREFIX_SCAN));
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

/// The scan variant for a marker found by [`find_container_marker`], with the probe short name its
/// size check stands for. `ftyp` sits four bytes into its atom, so the MP4 check starts there.
fn marker_kind(at: usize, marker: &[u8; 4]) -> (FirstMarker, &'static str) {
    match marker {
        b"fLaC" => (FirstMarker::Flac(at), expected_probe_format(marker)),
        b"caff" => (FirstMarker::Caf(at), expected_probe_format(marker)),
        b"ftyp" => (FirstMarker::Mp4(at.saturating_sub(4)), expected_probe_format(marker)),
        _ => (FirstMarker::Wav(at), expected_probe_format(marker)),
    }
}

/// The probe short name a container scan variant stands for.
fn marker_name(marker: FirstMarker) -> &'static str {
    match marker {
        FirstMarker::Flac(_) => "flac",
        FirstMarker::Caf(_) => "caf",
        FirstMarker::Mp4(_) => "isomp4",
        FirstMarker::Wav(_) => "wave",
        FirstMarker::Audio => "",
    }
}

/// Whether `rest` starts with one of `R`'s exact registered probe markers.
fn has_probe_marker<R: ProbeableFormat<'static>>(rest: &[u8]) -> bool {
    R::probe_data().iter().any(|data| data.spec.markers.iter().any(|marker| rest.starts_with(marker)))
}

/// Score `R` at `at` exactly as symphonia's private probe does: `Supported` wins, EOF and every
/// other scoring error mean "not this format", while a real I/O error aborts the probe.
fn scores_supported<R: Scoreable>(stream: &mut MediaSourceStream<'_>, at: usize, depth: u16) -> Result<bool> {
    stream.seek(SeekFrom::Start(u64::try_from(at).unwrap_or(u64::MAX)))?;
    match R::score(ScopedStream::new(stream, u64::from(depth))) {
        Ok(Score::Supported(_)) => Ok(true),
        Err(SymError::IoError(e)) if e.kind() != std::io::ErrorKind::UnexpectedEof => Err(AudioError::Malformed(e.to_string())),
        _ => Ok(false),
    }
}

/// Whether `b[at..]` is an MPEG/ADTS candidate the probe would actually select: an exact
/// `probe_data()` marker and a `Supported` score from the same reader. A loose `0xFFEx` pair is
/// not enough: `0xFFE0` is neither a registered marker nor a valid frame, so it must not stop the
/// scan before a later FLAC/CAF/MP4 candidate.
fn audio_sync_at<'a>(b: &'a [u8], at: usize, stream: &mut Option<MediaSourceStream<'a>>, budget: &mut ScoreBudget) -> Result<bool> {
    let rest = b.get(at..).unwrap_or_default();
    let adts = has_probe_marker::<symphonia::default::formats::AdtsReader<'static>>(rest);
    let mpa = has_probe_marker::<symphonia::default::formats::MpaReader<'static>>(rest);
    if !adts && !mpa {
        return Ok(false);
    }
    if stream.is_none() {
        *stream = Some(MediaSourceStream::new(Box::new(Cursor::new(b)), MediaSourceStreamOptions::default()));
    }
    let Some(mss) = stream.as_mut() else { return Ok(false) };
    let depth = ProbeOptions::default().max_score_depth;
    if adts && budget.take() && scores_supported::<symphonia::default::formats::AdtsReader<'static>>(mss, at, depth)? {
        return Ok(true);
    }
    if mpa && budget.take() && scores_supported::<symphonia::default::formats::MpaReader<'static>>(mss, at, depth)? {
        return Ok(true);
    }
    Ok(false)
}

/// Find the first marker that can start a container or audio stream. Symphonia's `Probe::probe`
/// constructs the reader itself and exposes no pre-construction hook, while checking every magic
/// in the scan window would validate bytes the probe never reaches (for example a valid MP3's
/// audio payload). Stopping at the first marker avoids that: it is the candidate the probe selects
/// for FLAC, CAF, MP4, Ogg and Matroska, and for WAV/AIFF once their full form is verified below.
/// MPEG/ADTS candidates are checked with the readers' own `probe_data()`/`Scoreable` APIs, so a
/// false sync byte pair cannot stop the scan. If that scoring budget runs out, the scan stops at
/// the next audio candidate instead of trusting a container-like byte string later in the unscored
/// audio payload. A container marker reached before any such candidate is still preflighted; if
/// the probe selects a size-checked container the scan did not check, `open` refuses it.
///
/// WAV candidates include RF64/BW64 so `check_info` can run, although symphonia 0.6 registers only
/// the RIFF marker. Native PCM RF64/BW64 is parsed by `wav::parse` before this path; a foreign
/// RF64/BW64 stream therefore remains unsupported by the probe rather than misdetected.
fn first_marker(b: &[u8], start: usize) -> Result<Option<FirstMarker>> {
    first_marker_with_budget(b, start, &mut ScoreBudget::new())
}

/// [`first_marker`] with an explicit score budget, so a test can prove a prefix crowded with
/// false sync markers performs only the bounded number of reader scores.
fn first_marker_with_budget(b: &[u8], start: usize, budget: &mut ScoreBudget) -> Result<Option<FirstMarker>> {
    let end = b.len().min(start.saturating_add(MAX_PREFIX_SCAN));
    let mut at = start;
    let mut audio_probe = None;
    while at.saturating_add(4) <= end {
        let rest = b.get(at..).unwrap_or_default();
        if rest.get(..4) == Some(b"fLaC".as_slice()) {
            return Ok(Some(FirstMarker::Flac(at)));
        }
        if rest.get(..4) == Some(b"caff".as_slice()) {
            return Ok(Some(FirstMarker::Caf(at)));
        }
        if rest.get(..4) == Some(b"OggS".as_slice()) || rest.get(..4) == Some(b"\x1A\x45\xDF\xA3".as_slice()) || crate::aiff::is_aiff(rest) {
            return Ok(Some(FirstMarker::Audio));
        }
        if crate::wav::is_wav(rest) {
            return Ok(Some(FirstMarker::Wav(at)));
        }
        if let (Some(&b0), Some(&b1)) = (b.get(at), b.get(at + 1))
            && b0 == 0xFF
            && b1 & 0xE0 == 0xE0
        {
            // A candidate after the cap cannot be scored. Stop rather than let a container-like
            // byte string later in an unscored audio payload become preflight-authoritative; the
            // probe will select the reader, and `open` refuses an unchecked size-checked one.
            if budget.exhausted() {
                return Ok(None);
            }
            if audio_sync_at(b, at, &mut audio_probe, budget)? {
                return Ok(Some(FirstMarker::Audio));
            }
        }
        if rest.get(4..8) == Some(b"ftyp".as_slice()) {
            return Ok(Some(FirstMarker::Mp4(at)));
        }
        at += 1;
    }
    Ok(None)
}

/// Run the early size checks on the container the probe can select. `start` is the first byte
/// after leading ID3v2 tags. A zero-frame FLAC is returned as an empty stream because Symphonia
/// 0.6's reader requires a first audio frame to resynchronize to.
fn precheck(b: &[u8], start: usize, detected: FileFormat) -> Result<Precheck> {
    let any_marker = |b: &[u8], start: usize| -> Result<Option<(FirstMarker, &'static str)>> {
        Ok(match first_marker(b, start)? {
            // Stop at the first marker of any kind, so container magic incidental to an audio
            // payload is not mistaken for the stream's container.
            Some(FirstMarker::Audio) | None => None,
            Some(marker) => Some((marker, marker_name(marker))),
        })
    };
    let found = match container_markers(detected) {
        // Follow the detection: check the first marker of the family it names, so a misleading
        // marker cannot let a differently sized container reach the probe unchecked. A detection
        // is only a hint, though: when the stream holds none of its markers (a FLAC behind a
        // `.mp3` name), the probe can still select another container, which is checked here.
        Some(markers) => match find_container_marker(b, start, markers) {
            Some((at, marker)) => Some(marker_kind(at, marker)),
            None => any_marker(b, start)?,
        },
        None => any_marker(b, start)?,
    };
    let Some((marker, expected)) = found else { return Ok(Precheck::Unchecked) };
    match marker {
        FirstMarker::Flac(at) => {
            crate::flac::check_metadata(b, at)?;
            if let Some(info) = crate::flac::empty_stream_info(b, at) {
                return Ok(Precheck::Empty(info));
            }
            Ok(Precheck::Checked(expected))
        }
        FirstMarker::Caf(at) => {
            crate::caf::check(b, at)?;
            Ok(Precheck::Checked(expected))
        }
        FirstMarker::Mp4(at) => {
            crate::mp4::check(b, at)?;
            Ok(Precheck::Checked(expected))
        }
        FirstMarker::Wav(at) => {
            crate::wav::check_info(b, at)?;
            Ok(Precheck::Checked(expected))
        }
        FirstMarker::Audio => Ok(Precheck::Unchecked),
    }
}

/// Skip leading ID3v2 tags, run the container size preflight, then probe with symphonia; a zero-frame FLAC found by
/// the preflight comes back as [`Opened::Empty`] without probing. A size-checked container is refused when probing
/// selects a different reader or one that was never checked, so incidental payload magic cannot bypass those limits.
fn open(bytes: &[u8], detected: FileFormat) -> Result<Opened> {
    let start = skip_id3v2(bytes);
    if start > 0 && start >= bytes.len() {
        return Err(AudioError::Malformed("ID3v2 tag extends past the end of the file".into()));
    }
    let checked = precheck(bytes, start, detected)?;
    if let Precheck::Empty(info) = checked {
        return Ok(Opened::Empty(info));
    }
    let mut mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), MediaSourceStreamOptions::default());
    // Seek (rather than pre-positioning the cursor) so the stream's absolute position is `start`
    // too: container offsets such as MP4's `stco` are absolute file offsets.
    mss.seek(SeekFrom::Start(u64::try_from(start).unwrap_or(u64::MAX)))?;
    let reader: Box<dyn FormatReader> =
        symphonia::default::get_probe().probe(&Hint::new(), mss, FormatOptions::default(), MetadataOptions::default()).map_err(map_err)?;
    let probed = reader.format_info().short_name;
    match checked {
        // Only containers whose readers allocate from sizes in the file need the check. A stream
        // the probe reads with another reader (MP3, Ogg, AIFF, ...) has no such size fields, so a
        // marker found in its audio data must not reject it.
        Precheck::Checked(expected) if SIZE_CHECKED_PROBE_FORMATS.contains(&probed) && probed != expected => {
            return Err(AudioError::Malformed(format!("the probed container ({probed}) is not the one whose size checks ran")));
        }
        Precheck::Unchecked if SIZE_CHECKED_PROBE_FORMATS.contains(&probed) => {
            return Err(AudioError::Malformed(format!("the probed container ({probed}) was not size-checked")));
        }
        _ => {}
    }
    Ok(Opened::Reader(reader))
}

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

/// The audio parameters of `track`, if it carries any.
fn audio_params(track: &Track) -> Option<&AudioCodecParameters> {
    track.codec_params.as_ref()?.audio()
}

/// The first track with a decodable audio codec, with the stream information the UI shows.
fn info_of(reader: &dyn FormatReader, detected: FileFormat) -> Result<(u32, AudioInfo)> {
    let track = reader
        .tracks()
        .iter()
        .find(|t| audio_params(t).is_some_and(|p| p.codec != CODEC_ID_NULL_AUDIO))
        .ok_or_else(|| AudioError::Unsupported("no decodable audio track".into()))?;
    let p = audio_params(track).ok_or_else(|| AudioError::Unsupported("no decodable audio track".into()))?;
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

/// A demux error ends the decode; it is only an error when nothing was decoded yet.
fn demux_failure(frames: u64, e: SymError) -> Result<()> {
    if frames == 0 {
        return Err(map_err(e));
    }
    log::warn!("stopping decode after demux error: {e}");
    Ok(())
}

/// Note a corrupt packet (the `errors`-th); a stream that never decodes is malformed.
fn corrupt_packet(errors: usize, frames: u64, msg: &str) -> Result<()> {
    log::debug!("skipping corrupt packet: {msg}");
    if errors > 1000 && frames == 0 {
        return Err(AudioError::Malformed(format!("no decodable packets: {msg}")));
    }
    Ok(())
}

/// The decoded stream is authoritative for the channel count: record it and allocate the planes.
fn init_planar(info: &mut AudioInfo, ch: usize) -> Result<Vec<Vec<f32>>> {
    info.channels = u16::try_from(ch).map_err(|_| AudioError::Unsupported("too many channels".into()))?;
    Ok((0..ch).map(|_| Vec::new()).collect())
}

/// Append one interleaved block to the planes, replacing non-finite samples with silence.
fn append_interleaved(planar: &mut [Vec<f32>], interleaved: &[f32], ch: usize) {
    for frame in interleaved.chunks_exact(ch) {
        for (dst, &s) in planar.iter_mut().zip(frame) {
            dst.push(if s.is_finite() { s } else { 0.0 });
        }
    }
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
    let track = reader.tracks().iter().find(|t| t.id == track_id).ok_or_else(|| AudioError::Malformed("track vanished".into()))?;
    if let Some(n) = track.num_frames {
        let total = n.saturating_mul(u64::from(info.channels.max(1)));
        if !count_only && total > MAX_DECODE_SAMPLES {
            return Err(AudioError::TooLarge(format!("{total} samples exceeds the decode limit")));
        }
    }
    let params = audio_params(track).cloned().ok_or_else(|| AudioError::Malformed("track vanished".into()))?;
    let alac = params.extra_data.as_deref().filter(|_| params.codec == CODEC_ID_ALAC).and_then(|d| be_u32(d, 0));
    if params.max_frames_per_packet.into_iter().chain(alac.map(u64::from)).any(|n| n > MAX_PACKET_FRAMES) {
        return Err(AudioError::Malformed("the file declares packets far longer than any codec uses".into()));
    }
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(&params, &decoder_options()).map_err(map_err)?;

    let mut planar: Vec<Vec<f32>> = Vec::new();
    let mut interleaved: Vec<f32> = Vec::new();
    let mut frames: u64 = 0;
    let mut errors = 0usize;
    loop {
        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                demux_failure(frames, e)?;
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
                corrupt_packet(errors, frames, m)?;
                continue;
            }
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(map_err(e)),
        };
        // The interleaved copy below is laid out by planes: use their count so a decoder whose
        // channel spec disagrees with its planes cannot trip the copy's length assertion.
        let ch = decoded.num_planes();
        if ch == 0 {
            continue;
        }
        if planar.is_empty() {
            planar = init_planar(&mut info, ch)?;
        }
        let n = decoded.frames() as u64;
        frames = frames.saturating_add(n);
        if frames.saturating_mul(ch as u64) > MAX_DECODE_SAMPLES {
            return Err(AudioError::TooLarge("decoded audio exceeds the 4 GiB limit".into()));
        }
        if count_only {
            continue;
        }
        let needed = usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_mul(ch))
            .ok_or_else(|| AudioError::TooLarge("decoded audio exceeds the 4 GiB limit".into()))?;
        if interleaved.len() < needed {
            interleaved.try_reserve(needed - interleaved.len()).map_err(|_| AudioError::TooLarge("cannot allocate decoded audio".into()))?;
            interleaved.resize(needed, 0.0);
        }
        decoded.copy_to_slice_interleaved(&mut interleaved[..needed]);
        append_interleaved(&mut planar, &interleaved[..needed], ch);
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

/// Frames symphonia's AAC decoder yields for every packet; it supports no other frame length.
const AAC_FRAMES_PER_PACKET: u64 = 1024;

/// Whether `track` of `reader` is AAC in MP4. The MP4 reader reports the stts total as the length,
/// but the decoder yields a whole frame per packet (sample durations are not applied), so a last
/// sample whose duration differs from a frame makes the two disagree.
fn is_mp4_aac(reader: &dyn FormatReader, track_id: u32) -> bool {
    reader.format_info().short_name == "isomp4"
        && reader.tracks().iter().find(|t| t.id == track_id).and_then(audio_params).is_some_and(|p| p.codec == CODEC_ID_AAC)
}

/// The decoded length of AAC in MP4: count the track's packets without decoding them.
fn mp4_aac_frames(reader: &mut dyn FormatReader, track_id: u32) -> Result<u64> {
    let mut frames: u64 = 0;
    loop {
        match reader.next_packet() {
            Ok(Some(packet)) if packet.track_id == track_id => frames = frames.saturating_add(AAC_FRAMES_PER_PACKET),
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                demux_failure(frames, e)?;
                break;
            }
        }
    }
    Ok(frames)
}

pub(crate) fn probe(bytes: &[u8], detected: FileFormat) -> Result<AudioInfo> {
    guarded(|| {
        let mut reader = match open(bytes, detected)? {
            Opened::Reader(reader) => reader,
            Opened::Empty(info) => return Ok(info),
        };
        let (track_id, mut info) = info_of(reader.as_ref(), detected)?;
        if info.frames > 0 && info.channels > 0 && is_mp4_aac(reader.as_ref(), track_id) {
            info.frames = mp4_aac_frames(reader.as_mut(), track_id)?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::codecs::audio::well_known::{
        CODEC_ID_AAC, CODEC_ID_ADPCM_MS, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_PCM_F32LE, CODEC_ID_PCM_F64LE, CODEC_ID_PCM_S16LE,
        CODEC_ID_VORBIS,
    };

    /// Build codec parameters for a test, optionally declaring a bit depth.
    fn params(codec: AudioCodecId, bits_per_sample: Option<u32>) -> AudioCodecParameters {
        let mut params = AudioCodecParameters::new();
        params.for_codec(codec);
        if let Some(bits) = bits_per_sample {
            params.with_bits_per_sample(bits);
        }
        params
    }

    /// Codec ids map to their reported formats, while a format detected from the container wins over the codec.
    #[test]
    fn codecs_map_to_the_reported_formats() {
        assert_eq!(format_from_codec(CODEC_ID_FLAC, FileFormat::Other), FileFormat::Flac);
        assert_eq!(format_from_codec(CODEC_ID_MP3, FileFormat::Other), FileFormat::Mp3);
        assert_eq!(format_from_codec(CODEC_ID_AAC, FileFormat::Other), FileFormat::Aac);
        assert_eq!(format_from_codec(CODEC_ID_ALAC, FileFormat::Other), FileFormat::Alac);
        assert_eq!(format_from_codec(CODEC_ID_VORBIS, FileFormat::Other), FileFormat::Ogg);
        // The container detected from the bytes wins over the codec.
        assert_eq!(format_from_codec(CODEC_ID_ALAC, FileFormat::Aac), FileFormat::Aac);
    }

    /// PCM and FLAC keep their declared bit depth; lossy codecs report `Compressed` regardless of their decoded
    /// width.
    #[test]
    fn sample_formats_keep_their_depth_and_lossy_marker() {
        assert_eq!(sample_format_of(&params(CODEC_ID_PCM_S16LE, Some(16))), SampleFormat::Int16);
        assert_eq!(sample_format_of(&params(CODEC_ID_FLAC, Some(24))), SampleFormat::Int24);
        assert_eq!(sample_format_of(&params(CODEC_ID_FLAC, Some(8))), SampleFormat::Int8);
        assert_eq!(sample_format_of(&params(CODEC_ID_FLAC, Some(32))), SampleFormat::Int32);
        assert_eq!(sample_format_of(&params(CODEC_ID_PCM_F32LE, Some(32))), SampleFormat::Float32);
        assert_eq!(sample_format_of(&params(CODEC_ID_PCM_F64LE, Some(64))), SampleFormat::Float64);
        // Lossy codecs report no fixed bit depth, whatever their decoded sample format is.
        assert_eq!(sample_format_of(&params(CODEC_ID_AAC, Some(16))), SampleFormat::Compressed);
        assert_eq!(sample_format_of(&params(CODEC_ID_ADPCM_MS, Some(4))), SampleFormat::Compressed);
        assert_eq!(sample_format_of(&params(CODEC_ID_ALAC, None)), SampleFormat::Compressed);
    }

    /// Decoder options keep gapless trimming enabled so the encoder delay and padding reported by the demuxer are
    /// removed.
    #[test]
    fn gapless_decoding_stays_enabled() {
        assert!(decoder_options().gapless);
    }

    /// A complete MPEG frame header is a candidate; loose or unregistered sync pairs are rejected before scoring,
    /// while a registered but truncated header is rejected by the reader's score after consuming a budget slot.
    #[test]
    fn only_the_readers_own_audio_markers_are_candidates() {
        /// Whether the preflight would score `b` as an audio candidate at offset 0.
        fn candidate(b: &[u8]) -> bool {
            let mut stream = None;
            let mut budget = ScoreBudget::new();
            matches!(audio_sync_at(b, 0, &mut stream, &mut budget), Ok(true))
        }
        let mut mp3_frame = vec![0u8; 417];
        mp3_frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // MPEG-1 Layer III, 128 kbps, 44.1 kHz
        assert!(candidate(&[&mp3_frame[..], &mp3_frame].concat()));
        for false_sync in [vec![0xFF, 0xE0, 0x00], vec![0xFF, 0xF0, 0x00], vec![0xFF, 0xFB, 0xF0, 0x00]] {
            assert!(!candidate(&false_sync), "{false_sync:?} is not a probe candidate");
        }
    }

    /// A prefix made of false MPEG sync markers must not ask the readers to score every byte
    /// position: the per-scan budget caps the expensive work. A cheap container marker reached
    /// before the next audio candidate is still found, but once the cap is spent an audio
    /// candidate stops the scan so later payload magic cannot be preflight-authoritative.
    #[test]
    fn a_prefix_crowded_with_sync_markers_scores_only_the_budget() {
        let mut crowded = vec![0u8; 1 << 16];
        for pair in crowded.as_chunks_mut::<2>().0 {
            pair.copy_from_slice(&[0xFF, 0xE2]);
        }
        let mut budget = ScoreBudget::new();
        assert!(matches!(first_marker_with_budget(&crowded, 0, &mut budget), Ok(None)));
        assert_eq!(budget.used, MAX_AUDIO_SCORE_ATTEMPTS, "no candidate after the cap is scored");

        // At the cap boundary, with no later audio candidate, the cheap marker is still found.
        let mut at_cap: Vec<u8> = (0..MAX_AUDIO_SCORE_ATTEMPTS).flat_map(|_| [0xFFu8, 0xE2]).collect();
        at_cap.extend_from_slice(b"fLaC");
        let mut budget = ScoreBudget::new();
        assert!(
            matches!(first_marker_with_budget(&at_cap, 0, &mut budget), Ok(Some(FirstMarker::Flac(_)))),
            "a container marker reached before the next audio candidate is still found"
        );
        assert_eq!(budget.used, MAX_AUDIO_SCORE_ATTEMPTS);

        // One more false candidate between the cap and the magic makes the scan stop unchecked:
        // the same magic could instead sit in a valid audio payload.
        let mut after_candidate = at_cap[..2 * MAX_AUDIO_SCORE_ATTEMPTS].to_vec();
        after_candidate.extend_from_slice(&[0xFF, 0xE2]);
        after_candidate.extend_from_slice(b"fLaC");
        let mut budget = ScoreBudget::new();
        assert!(matches!(first_marker_with_budget(&after_candidate, 0, &mut budget), Ok(None)), "an audio candidate after the cap stops the scan");
        assert_eq!(budget.used, MAX_AUDIO_SCORE_ATTEMPTS);
    }

    /// ID3v2 sizes are read in their 7-bit syncsafe form; consecutive tags are skipped by their declared sizes, and
    /// the v2.3 reserved footer bit does not extend a tag.
    #[test]
    fn leading_id3_tags_are_skipped_by_their_declared_size() {
        let tag = [&b"ID3\x04\0\0\0\0\0\x05"[..], b"hello"].concat();
        assert_eq!(skip_id3v2(&[&tag[..], &tag[..]].concat()), 30);
        assert_eq!(skip_id3v2(b"fLaC"), 0);
        // The 10-byte footer flag exists only in v2.4; the same bit in v2.3 is reserved.
        assert_eq!(crate::id3v2_len(b"ID3\x03\0\x10\0\0\0\0"), Some(10));
        assert_eq!(crate::id3v2_len(b"ID3\x04\0\x10\0\0\0\0"), Some(20));
    }
}
