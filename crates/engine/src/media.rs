//! Audio identity and explicit, validated relinking. All decoding/hashing runs off the callback.

use crate::{Engine, EngineError, Result};
use sha2::{Digest, Sha256};
use soundcraft_audio_io::AudioBuffer;
use soundcraft_model::{Source, SourceAudio, SourceId};
use std::path::Path;
use std::sync::Arc;

const IDENTITY_PREFIX: &str = "pcm-sha256-v1:";
const MAX_FILE_BYTES: u64 = 1 << 30;
const MAX_SAMPLE_VALUES: usize = 1 << 30;

/// SHA-256 of exact decoded f32 bits in planar channel order, with LE dimensions/lengths.
/// No quantization, normalization or clamping: different PCM must not become a matching identity.
pub fn audio_identity(buf: &AudioBuffer) -> String {
    let mut hash = Sha256::new();
    hash.update(b"soundcraft-pcm-v1\0");
    hash.update(buf.sample_rate.to_le_bytes());
    hash.update((buf.num_channels() as u64).to_le_bytes());
    hash.update((buf.frames() as u64).to_le_bytes());
    for channel in &buf.channels {
        hash.update((channel.len() as u64).to_le_bytes());
        let mut chunks = channel.chunks_exact(1024);
        let mut bytes = [0u8; 4096];
        for chunk in chunks.by_ref() {
            for (encoded, sample) in bytes.chunks_exact_mut(4).zip(chunk) {
                encoded.copy_from_slice(&sample.to_bits().to_le_bytes());
            }
            hash.update(bytes.as_slice());
        }
        for sample in chunks.remainder() {
            hash.update(sample.to_bits().to_le_bytes());
        }
    }
    format!("{IDENTITY_PREFIX}{:x}", hash.finalize())
}

/// Bounded deterministic conversion used before comparing or assigning a PCM identity.
pub(crate) fn at_rate(mut buf: AudioBuffer, rate: u32) -> std::result::Result<AudioBuffer, String> {
    if rate == 0 || buf.sample_rate == 0 || buf.frames() == 0 || buf.num_channels() == 0 {
        return Err("selected file has no usable audio or sample rate".into());
    }
    if buf.channels.iter().any(|channel| channel.len() != buf.frames()) {
        return Err("selected audio channels have different frame counts".into());
    }
    let frames = (buf.frames() as f64 * f64::from(rate) / f64::from(buf.sample_rate)).round();
    if frames * buf.num_channels() as f64 > MAX_SAMPLE_VALUES as f64
        || (rate != buf.sample_rate && frames > soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as f64)
    {
        return Err("selected audio is too large after sample-rate conversion".into());
    }
    if buf.sample_rate != rate {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, rate);
        buf.sample_rate = rate;
    }
    if buf.frames() == 0 {
        return Err("selected file has no usable audio at the source sample rate".into());
    }
    Ok(buf)
}

/// Validate source bounds/identity before admitting decoded audio to its existing clips.
pub(crate) fn validate_audio(src: &Source, buf: &AudioBuffer, allow_unverified: bool) -> std::result::Result<(), String> {
    let identity = src.audio_identity.as_ref().map(|_| audio_identity(buf));
    validate_identity(src, buf, identity.as_deref(), allow_unverified)
}

/// Validate against a cached PCM identity so sources sharing a file do not repeatedly hash it.
pub(crate) fn validate_identity(src: &Source, buf: &AudioBuffer, identity: Option<&str>, allow_unverified: bool) -> std::result::Result<(), String> {
    if buf.num_channels() != usize::from(src.channels) || buf.frames() as u64 != src.frames || buf.sample_rate != src.sample_rate {
        return Err(format!(
            "audio metadata differs: expected {} channels, {} frames at {} Hz; selected {} channels, {} frames at {} Hz",
            src.channels,
            src.frames,
            src.sample_rate,
            buf.num_channels(),
            buf.frames(),
            buf.sample_rate
        ));
    }
    match &src.audio_identity {
        Some(expected) if !expected.starts_with(IDENTITY_PREFIX) => Err("unsupported audio identity version; cannot verify this source".into()),
        Some(expected) if identity != Some(expected.as_str()) => Err("audio content identity differs; this is not the original audio".into()),
        None if !allow_unverified => Err(
            "this legacy source has no saved audio identity; matching metadata cannot prove its content. Confirm allow_unverified explicitly to use it"
                .into(),
        ),
        _ => Ok(()),
    }
}

/// User-selected audio only: loaded media cannot be replaced accidentally, even with consent.
pub fn relink(e: &mut Engine, id: SourceId, path: &str, allow_unverified: bool) -> Result<serde_json::Value> {
    if cfg!(target_arch = "wasm32") {
        return Err(EngineError::Unsupported("audio relinking by filesystem path requires the native app".into()));
    }
    let bad = |message: String| EngineError::BadParams("media.relink".into(), message);
    let src = e.session().source(id).cloned().ok_or_else(|| bad("unknown audio source".into()))?;
    if e.session().pool.contains(id) {
        return Err(bad("audio is already loaded; relink only restores missing audio".into()));
    }
    if src.sample_rate != e.session().sample_rate.hz() {
        return Err(bad("source and session rates differ; relink cannot change existing clip timing".into()));
    }
    let path = Path::new(path);
    let (bytes, resolved) = crate::io::read_relink_candidate(path, MAX_FILE_BYTES)?;
    let (info, buf) = soundcraft_audio_io::decode(&bytes, path.extension().and_then(|x| x.to_str()))
        .map_err(|error| EngineError::Io(format!("{}: {error}", path.display())))?;
    let buf = at_rate(buf, src.sample_rate).map_err(bad)?;
    validate_audio(&src, &buf, allow_unverified).map_err(bad)?;
    let verified = src.audio_identity.is_some();
    let identity = src.audio_identity.clone().unwrap_or_else(|| audio_identity(&buf));
    let audio = Arc::new(SourceAudio::new(buf));
    // No session mutation or undo entry until every candidate check succeeds.
    let s = e.session_mut();
    let target = s.sources.iter_mut().find(|source| source.id == id).ok_or_else(|| bad("unknown audio source".into()))?;
    target.path = resolved.clone();
    target.format = info.format;
    target.unsaved = false;
    target.audio_identity = Some(identity);
    s.pool.insert(id, audio);
    Ok(serde_json::json!({"source": id.0, "path": resolved, "identity_verified": verified, "legacy_confirmation": !verified}))
}
