use super::render::sanitize_name;
use super::{
    Arc, AudioBuffer, ChannelFormat, Clip, Engine, EngineError, FileFormat, OsStr, Path, PathBuf, Result, Samples, Session, Source, SourceAudio,
    SourceId, TrackId, TrackKind, Value, file_found, file_key, json, read_file, stored_path,
};

/// Largest import we accept (samples per channel × channels), ~4 GiB of f32.
pub(crate) const MAX_IMPORT_SAMPLES: usize = 1 << 30;

/// `buf` converted to `rate`, unless it would then hold more than `limit` samples: a file with a
/// tiny sample rate grows by orders of magnitude.
pub(crate) fn at_rate(mut buf: AudioBuffer, rate: u32, limit: usize) -> Option<AudioBuffer> {
    let from = buf.sample_rate.max(1);
    if buf.frames() as f64 * buf.num_channels() as f64 * f64::from(rate) / f64::from(from) > limit as f64 {
        return None;
    }
    if from != rate {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, from, rate);
        buf.sample_rate = rate;
    }
    Some(buf)
}

/// A relative `Audio Files/` path no other source in `s` already uses.
fn unique_audio_rel(s: &Session, stem: &str) -> String {
    let stem = sanitize_name(stem);
    let (plain, prefix) = (format!("Audio Files/{stem}.wav"), format!("Audio Files/{stem}-"));
    let used: std::collections::HashSet<u64> = s
        .sources
        .iter()
        .filter_map(|src| {
            if src.path == plain {
                return Some(1);
            }
            let n = src.path.strip_prefix(&prefix)?.strip_suffix(".wav")?;
            n.parse().ok().filter(|i: &u64| *i > 1 && i.to_string() == n)
        })
        .collect();
    match (1..).find(|i| !used.contains(i)) {
        Some(n) if n > 1 => format!("{prefix}{n}.wav"),
        _ => plain,
    }
}

/// Register decoded audio as a new source. Resamples to the session rate if needed.
pub fn add_source(s: &mut Session, name: &str, mut buf: AudioBuffer, path: Option<&str>, format: FileFormat) -> SourceId {
    let sr = s.sample_rate.hz();
    if buf.sample_rate != sr && buf.sample_rate > 0 {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, sr);
        buf.sample_rate = sr;
    }
    add_source_audio(s, name, Arc::new(SourceAudio::new(buf)), path, format)
}

/// Register audio already at the session rate, with its peaks built, as a new source.
fn add_source_audio(s: &mut Session, name: &str, audio: Arc<SourceAudio>, path: Option<&str>, format: FileFormat) -> SourceId {
    let id = SourceId(s.alloc());
    let stem = Path::new(name).file_stem().and_then(OsStr::to_str).unwrap_or(name).to_string();
    let stored = match path {
        Some(p) => p.to_string(),
        None => unique_audio_rel(s, &stem),
    };
    s.sources.push(Source {
        id,
        name: stem.clone(),
        path: stored,
        channels: u16::try_from(audio.buffer.num_channels()).unwrap_or(u16::MAX),
        frames: audio.frames() as u64,
        sample_rate: s.sample_rate.hz(),
        format,
        time_reference: 0,
        unsaved: true,
    });
    s.pool.insert(id, audio);
    id
}

/// A BWF time stamp (samples at the file's rate) at the session's rate, as the audio is resampled.
fn stamp_at_rate(stamp: u64, file_rate: u32, session_rate: u32) -> u64 {
    if file_rate == 0 || file_rate == session_rate {
        return stamp;
    }
    let (from, to) = (u128::from(file_rate), u128::from(session_rate));
    u64::try_from((u128::from(stamp) * to + from / 2) / from).unwrap_or(u64::MAX)
}

/// An audio file decoded, resampled to a session rate and given its waveform peaks: everything an
/// import costs except placing the clip. Making one needs no engine, so the app makes it on a
/// worker thread ([`decode_import`]) and hands it over with [`Engine::offer_import`] before it
/// runs `file.import_audio`, which then only places the clip (issue #597).
#[derive(Debug, Clone)]
pub struct DecodedImport {
    pub(crate) key: PathBuf,
    pub(crate) rate: u32,
    info: soundcraft_audio_io::AudioInfo,
    audio: Arc<SourceAudio>,
}

impl DecodedImport {
    /// Frames at the session rate it was decoded for.
    pub fn frames(&self) -> usize {
        self.audio.frames()
    }
}

/// Reads and decodes the audio file at `path` for a session at `rate`. Thread-safe and slow (the
/// whole file is read, decoded, resampled and given peaks): run it off the UI thread.
pub fn decode_import(path: &str, rate: u32) -> Result<DecodedImport> {
    let bytes = read_file(path)?;
    let name = Path::new(path).file_name().and_then(OsStr::to_str).unwrap_or("Audio");
    let (info, audio) = decode_import_bytes(name, &bytes, rate)?;
    Ok(DecodedImport { key: file_key(Path::new(path)), rate, info, audio })
}

fn decode_import_bytes(name: &str, bytes: &[u8], rate: u32) -> Result<(soundcraft_audio_io::AudioInfo, Arc<SourceAudio>)> {
    let ext = Path::new(name).extension().and_then(OsStr::to_str).map(str::to_ascii_lowercase);
    let (info, buf) = soundcraft_audio_io::decode(bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    let buf = at_rate(buf, rate, MAX_IMPORT_SAMPLES).ok_or_else(|| EngineError::Io(format!("{name}: file is too large to import")))?;
    if buf.frames() == 0 {
        return Err(EngineError::Io(format!("{name}: no audio")));
    }
    Ok((info, Arc::new(SourceAudio::new(buf))))
}

/// Decode and place an audio file. With no target track, a new track of matching width is made.
pub fn import_audio_bytes(e: &mut Engine, name: &str, bytes: &[u8], path: Option<&str>, target: Option<TrackId>, at: Samples) -> Result<Value> {
    let (info, audio) = decode_import_bytes(name, bytes, e.session().sample_rate.hz())?;
    place_import(e, name, &info, audio, path, target, at)
}

/// Place audio from [`decode_import`] (made for this session's rate) as `import_audio_bytes` does.
pub(crate) fn place_decoded(e: &mut Engine, name: &str, d: DecodedImport, path: Option<&str>, target: Option<TrackId>, at: Samples) -> Result<Value> {
    place_import(e, name, &d.info, d.audio, path, target, at)
}

fn place_import(
    e: &mut Engine,
    name: &str,
    info: &soundcraft_audio_io::AudioInfo,
    audio: Arc<SourceAudio>,
    path: Option<&str>,
    target: Option<TrackId>,
    at: Samples,
) -> Result<Value> {
    let channels = audio.buffer.num_channels();
    let file_rate = info.sample_rate;
    let s = e.session_mut();
    let session_rate = s.sample_rate.hz();
    let src = add_source_audio(s, name, audio, path, info.format);
    if let (Some(bwf), Some(sx)) = (info.bwf.as_ref(), s.sources.last_mut()) {
        sx.time_reference = stamp_at_rate(bwf.time_reference, file_rate, session_rate);
    }
    let frames = s.source(src).map_or(0, |x| i64::try_from(x.frames).unwrap_or(0));
    let track = match target {
        Some(t) => t,
        None => {
            let stem = Path::new(name).file_stem().and_then(OsStr::to_str).unwrap_or("Audio").to_string();
            s.add_track(TrackKind::Audio, ChannelFormat::for_channels(channels), Some(&stem))
        }
    };
    let cid = s.new_clip_id();
    let stem = s.source(src).map(|x| x.name.clone()).unwrap_or_default();
    crate::edit::place_clip(s, track, Clip::audio(cid, stem, src, 0, at, frames));
    Ok(json!({"track": track, "clip": cid, "source": src, "frames": frames, "channels": channels}))
}

/// Loads an audio file's bytes into an instrument track's Sampler as one undo step ("Load
/// Sampler Sample"): for files that have no path, such as one dropped into the browser. The same
/// checks as `mix.sampler_load`; not journaled, since the bytes can be large.
pub fn load_sampler_bytes(e: &mut Engine, track: TrackId, name: &str, bytes: &[u8]) -> Result<Value> {
    let before = e.session_arc();
    let r = crate::cmd::sampler::attach(e, track, name, bytes, None, Vec::new())?;
    e.push_undo("Load Sampler Sample", before);
    Ok(r)
}

/// Resolve each movie referenced by the session file `session` ([`stored_path`]): keep the
/// resolved path when its file exists, else look for a file of the same name next to the session
/// (moved projects) and relink. A movie whose stored path is refused and has no such copy is
/// marked offline. Returns the movies still missing.
pub fn relink_videos(s: &mut Session, session: &Path) -> Vec<String> {
    let session_dir = session.parent().unwrap_or(Path::new(""));
    let mut missing = Vec::new();
    for v in &mut s.videos {
        v.offline = None;
        let p = stored_path(&v.path, session);
        // The UI opens `path` as it is: keep the checked path, not one relative to the working
        // directory.
        if let Ok(found) = &p
            && file_found(found)
        {
            v.path = found.to_string_lossy().into_owned();
            continue;
        }
        let name = Path::new(&v.path).file_name().map(PathBuf::from).unwrap_or_default();
        let near = [session_dir.join(&name), session_dir.join("Video Files").join(&name)]
            .into_iter()
            .find(|c| !name.as_os_str().is_empty() && file_found(c));
        match (near, p) {
            (Some(c), _) => v.path = c.to_string_lossy().into_owned(),
            (None, Ok(p)) => missing.push(format!("{} (video)", p.display())),
            (None, Err(why)) => {
                missing.push(format!("{} (video): {why}", v.path));
                v.offline = Some(why);
            }
        }
    }
    missing
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    #[test]
    fn a_tiny_sample_rate_is_refused_before_it_is_converted() {
        let buf = |rate| AudioBuffer { sample_rate: rate, channels: vec![vec![0.5; 100]; 2] };
        assert!(at_rate(buf(1), 48_000, 1_000_000).is_none());
        let ok = at_rate(buf(8_000), 48_000, 1_000_000).unwrap();
        assert_eq!((ok.sample_rate, ok.frames()), (48_000, 600));
    }
}
