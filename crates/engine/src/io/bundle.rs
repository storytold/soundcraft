use super::render::sanitize_name;
use super::session::decode_source;
use super::{Arc, Engine, EngineError, OsStr, Path, Result, Session, SourceAudio, read_bundle, session_limit, write_file};

// ─── Hosts without a filesystem (the browser) ─────────────────────────────────────────────────

/// Extension of a session bundle: one zip file holding the `.scraft` session and its
/// `Audio Files/`, the same layout as a saved session folder. The browser has no folders, so it
/// saves and opens sessions as bundles.
pub const BUNDLE_EXTENSION: &str = craft_scraft_format::BUNDLE_EXTENSION;

/// Most entries and bytes a bundle may unpack to (bundles are hostile input). The writer keeps to
/// them too, so a bundle it saves always opens again (#433).
#[derive(Clone, Copy, Debug)]
pub(super) struct BundleLimits {
    pub(super) entries: usize,
    pub(super) bytes: u64,
}

const BUNDLE_LIMITS: BundleLimits = BundleLimits { entries: 10_000, bytes: 1 << 30 };

/// The session as a bundle: `<name>.scraft` plus every source's audio as 32-bit float WAV under
/// `Audio Files/` (stored, not compressed: audio barely compresses). Returns the bytes and the
/// session with its source paths pointing into the bundle.
pub fn session_bundle(session: &Session) -> Result<(Vec<u8>, Session)> {
    session_bundle_within(session, BUNDLE_LIMITS)
}

/// [`session_bundle`], refused when it would hold more than `limits` allows.
fn session_bundle_within(session: &Session, limits: BundleLimits) -> Result<(Vec<u8>, Session)> {
    let shared = craft_scraft_format::BundleLimits { entries: limits.entries, bytes: limits.bytes, session_bytes: session_limit() };
    craft_scraft_format::write_bundle(session, shared).map_err(|error| EngineError::Io(error.to_string()))
}

/// Save as a bundle named after `path`'s file name, delivered through the host ([`write_file`]).
pub(super) fn save_session_bundle(e: &mut Engine, path: &str) -> Result<usize> {
    let stem = Path::new(path).file_stem().and_then(OsStr::to_str).filter(|x| !x.is_empty()).map(str::to_string);
    let mut s = e.session().clone();
    if let Some(stem) = stem {
        s.name = stem;
    }
    let (bytes, fresh) = session_bundle(&s)?;
    let file = format!("{}.{BUNDLE_EXTENSION}", sanitize_name(&fresh.name));
    write_file(&file, &bytes)?;
    let written = fresh.sources.iter().filter(|x| fresh.pool.get(x.id).is_some()).count();
    *e.session_mut() = fresh;
    e.path = Some(file);
    e.mark_clean();
    Ok(written)
}

/// Open a session from bytes (the browser has no paths): a bundle ([`session_bundle`]) or a bare
/// `.scraft`, whose audio is then reported missing. Returns the missing media, like
/// [`open_session`].
pub fn open_session_bytes(e: &mut Engine, name: &str, bytes: &[u8]) -> Result<Vec<String>> {
    let (text, mut files) = if bytes.starts_with(b"PK\x03\x04") {
        read_bundle(bytes, BUNDLE_LIMITS)?
    } else {
        (super::session_bytes_text(name, bytes)?, std::collections::HashMap::new())
    };
    let mut s = Session::from_json(&text).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    let mut missing = Vec::new();
    // Several sources may share one file (native sessions do): decode each path once and share the result.
    let mut decoded: std::collections::HashMap<String, Option<Arc<SourceAudio>>> = std::collections::HashMap::new();
    for src in &s.sources {
        if let Some(audio) = decoded.get(&src.path) {
            if let Some(audio) = audio {
                s.pool.insert(src.id, Arc::clone(audio));
            }
            continue;
        }
        let audio = match files.remove(&src.path) {
            Some(bytes) => match decode_source(&bytes, Path::new(&src.path).extension().and_then(OsStr::to_str), s.sample_rate.hz()) {
                Ok(audio) => Some(audio),
                Err(err) => {
                    missing.push(format!("{}: {err}", src.path));
                    None
                }
            },
            None => {
                missing.push(src.path.clone());
                None
            }
        };
        if let Some(audio) = &audio {
            s.pool.insert(src.id, Arc::clone(audio));
        }
        decoded.insert(src.path.clone(), audio);
    }
    // Movies are referenced by path, never copied into a session: none can be found here.
    missing.extend(s.videos.iter().map(|v| v.path.clone()));
    let stem = Path::new(name).file_stem().and_then(OsStr::to_str).filter(|x| !x.is_empty()).unwrap_or("Untitled").to_string();
    e.replace_session(s);
    e.path = Some(format!("{}.{BUNDLE_EXTENSION}", sanitize_name(&stem)));
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    Ok(missing)
}

#[cfg(test)]
mod bundle_tests {
    use super::*;

    #[test]
    fn a_session_too_large_to_parse_is_refused_from_bytes() {
        // #892: a session the browser hands over as bytes has the cap a session file has. The
        // bytes are zeroed on allocation and never read, so they take no memory.
        let huge = vec![0u8; usize::try_from(super::super::MAX_SESSION_BYTES).unwrap() + 1];
        let err = open_session_bytes(&mut Engine::default(), "Huge.scraft", &huge).unwrap_err().to_string();
        assert_eq!(err, "Huge.scraft is too large to open (over 256 MiB)");
    }

    #[test]
    fn bundles_and_bare_sessions_keep_to_the_session_limit_both_ways() {
        // #892, with the limit lowered for this test: a bundle is saved only if its session can
        // be opened again (#433), and its reader and the bare-bytes reader refuse one past it.
        let e = crate::demo::demo_engine();
        let (bundle, fresh) = session_bundle(e.session()).unwrap();
        let inside = fresh.to_json().unwrap().len() as u64;
        let bare = e.session().to_json().unwrap();
        {
            let _limit = super::super::SessionLimit::set(inside.max(bare.len() as u64));
            assert!(session_bundle(e.session()).is_ok());
            assert!(open_session_bytes(&mut Engine::default(), "Song.scraftz", &bundle).is_ok());
            assert!(open_session_bytes(&mut Engine::default(), "Song.scraft", bare.as_bytes()).is_ok());
        }
        let _limit = super::super::SessionLimit::set(inside.min(bare.len() as u64) - 1);
        let err = session_bundle(e.session()).unwrap_err().to_string();
        assert!(err.starts_with("the session is too large to save"), "{err}");
        let err = open_session_bytes(&mut Engine::default(), "Song.scraftz", &bundle).unwrap_err().to_string();
        assert!(err.starts_with("bundle: ") && err.contains(".scraft is too large to open"), "{err}");
        let err = open_session_bytes(&mut Engine::default(), "Song.scraft", bare.as_bytes()).unwrap_err().to_string();
        assert!(err.starts_with("Song.scraft is too large to open"), "{err}");
    }

    #[test]
    fn bundle_round_trips_session_and_audio() {
        let mut e = crate::demo::demo_engine();
        e.execute("mix.volume", &serde_json::json!({"track": "Kick", "db": -7.0})).unwrap();
        let (bytes, fresh) = session_bundle(e.session()).unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"));
        assert!(fresh.sources.iter().all(|s| s.path.starts_with("Audio Files/") && !s.unsaved));
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Midnight Groove.scraftz", &bytes).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(r.session().track_by_name("Kick").unwrap().mixer.volume_db, -7.0);
        assert_eq!(r.session().tracks.len(), e.session().tracks.len());
        for src in &e.session().sources {
            let (a, b) = (e.session().pool.get(src.id).unwrap(), r.session().pool.get(src.id).unwrap());
            assert_eq!(a.buffer.channels, b.buffer.channels, "{}: float WAV keeps every sample", src.name);
        }
        assert!(!r.is_dirty());
        assert_eq!(r.path.as_deref(), Some("Midnight Groove.scraftz"));
    }

    #[test]
    fn legacy_zip64_bundle_opens_with_shared_reader() {
        use craft_audio::{AudioBuffer, EncodeFormat, EncodeOptions, encode};
        use soundcraft_model::{FileFormat, Source, SourceId};
        use std::io::Write as _;

        let id = SourceId(7);
        let samples = [0.125, -0.75, 1.25, -1.5];
        let mut audio = AudioBuffer::new(48_000, 1, samples.len());
        audio.channels[0].copy_from_slice(&samples);

        let mut session = Session::default();
        session.sources.push(Source {
            id,
            name: "Legacy Lead".into(),
            path: "Audio Files/Legacy Lead.wav".into(),
            channels: 1,
            frames: samples.len() as u64,
            sample_rate: 48_000,
            format: FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        session.pool.insert(id, Arc::new(SourceAudio::new(audio)));

        let wav =
            encode(&session.pool.get(id).unwrap().buffer, &EncodeOptions::new(EncodeFormat::Wav).with_bit_depth(craft_audio::BitDepth::Float32))
                .unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        zip.start_file("Audio Files/Legacy Lead.wav", opts).unwrap();
        zip.write_all(&wav).unwrap();
        zip.start_file("Legacy.scraft", opts).unwrap();
        zip.write_all(session.to_json().unwrap().as_bytes()).unwrap();
        let legacy = zip.finish().unwrap().into_inner();

        let mut opened = Engine::default();
        let missing = open_session_bytes(&mut opened, "Legacy.scraftz", &legacy).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(opened.session().pool.get(id).unwrap().buffer.channels[0], samples);
    }

    #[test]
    fn sources_sharing_one_file_all_load_from_a_bundle() {
        use std::io::Write as _;
        let e = crate::demo::demo_engine();
        let (bytes, _) = session_bundle(e.session()).unwrap();
        let (text, files) = read_bundle(&bytes, BUNDLE_LIMITS).unwrap();
        let mut s = Session::from_json(&text).unwrap();
        s.sources[1].path = s.sources[0].path.clone();
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for path in s.sources.iter().map(|x| &x.path).collect::<std::collections::BTreeSet<_>>() {
            w.start_file(path.as_str(), opts).unwrap();
            w.write_all(&files[path]).unwrap();
        }
        w.start_file("Song.scraft", opts).unwrap();
        w.write_all(s.to_json().unwrap().as_bytes()).unwrap();
        let bundle = w.finish().unwrap().into_inner();
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Song.scraftz", &bundle).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        for src in &r.session().sources {
            assert!(r.session().pool.get(src.id).is_some(), "{} has no audio", src.name);
        }
    }

    #[test]
    fn entries_sharing_one_payload_are_refused() {
        use std::io::Write as _;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        w.start_file("Song.scraft", opts).unwrap();
        w.write_all(Session::default().to_json().unwrap().as_bytes()).unwrap();
        w.start_file("Audio Files/a.wav", opts).unwrap();
        w.write_all(&[7u8; 4096]).unwrap();
        for i in 0..100 {
            w.shallow_copy_file("Audio Files/a.wav", &format!("Audio Files/copy{i}.wav")).unwrap();
        }
        let bundle = w.finish().unwrap().into_inner();
        let err = read_bundle(&bundle, BUNDLE_LIMITS).unwrap_err();
        assert!(err.to_string().contains("share data"), "{err}");
    }

    #[test]
    fn a_bundle_is_saved_only_if_it_opens_again() {
        // #433: the writer keeps to the reader's limits.
        let e = crate::demo::demo_engine();
        let (bytes, _) = session_bundle(e.session()).unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let entries = z.len();
        let size: u64 = (0..entries).map(|i| z.by_index(i).unwrap().size()).sum();
        let exact = BundleLimits { entries, bytes: size };
        let (at_limit, _) = session_bundle_within(e.session(), exact).unwrap();
        assert!(read_bundle(&at_limit, exact).is_ok(), "what the writer takes, the reader opens");
        assert!(session_bundle_within(e.session(), BundleLimits { entries: entries - 1, ..exact }).is_err());
        assert!(session_bundle_within(e.session(), BundleLimits { bytes: size - 1, ..exact }).is_err());
    }

    #[test]
    fn same_named_sources_get_distinct_files() {
        let mut e = crate::demo::demo_engine();
        let name = e.session().sources[0].name.clone();
        let s = e.session_mut();
        let n = s.sources.len();
        s.sources[1].name = name;
        let (bytes, fresh) = session_bundle(e.session()).unwrap();
        let paths: std::collections::HashSet<_> = fresh.sources.iter().map(|s| s.path.clone()).collect();
        assert_eq!(paths.len(), n);
        let mut r = Engine::default();
        assert!(open_session_bytes(&mut r, "x.scraftz", &bytes).unwrap().is_empty());
    }

    #[test]
    fn bare_session_opens_with_its_audio_reported_missing() {
        let e = crate::demo::demo_engine();
        let (_, fresh) = session_bundle(e.session()).unwrap();
        let text = fresh.to_json().unwrap();
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Song.scraft", text.as_bytes()).unwrap();
        assert_eq!(missing.len(), fresh.sources.len());
        assert_eq!(r.session().tracks.len(), fresh.tracks.len());
    }

    #[test]
    fn hostile_bundles_are_errors_not_panics() {
        let mut r = Engine::default();
        for bad in [&b"PK\x03\x04 not really a zip"[..], b"", b"{ not json", b"\xff\xfe\x00"] {
            assert!(open_session_bytes(&mut r, "bad.scraftz", bad).is_err());
        }
        // A valid zip without a session inside.
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("Audio Files/a.wav", zip::write::SimpleFileOptions::default()).unwrap();
        let only_audio = w.finish().unwrap().into_inner();
        assert!(open_session_bytes(&mut r, "a.scraftz", &only_audio).is_err());
        assert_eq!(r.session().tracks.len(), 0, "a failed open leaves the document alone");
    }

    #[test]
    fn write_file_writes_to_disk_natively() {
        let p = std::env::temp_dir().join(format!("soundcraft-write-file-{}.txt", std::process::id()));
        write_file(&p.to_string_lossy(), b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        let _ = std::fs::remove_file(&p);
    }
}
