use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::{Engine, io, media};
use soundcraft_model::{Clip, SourceId};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("soundcraft-relink-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wav(level: f32, channels: usize, frames: usize, rate: u32) -> Vec<u8> {
    soundcraft_audio_io::encode(
        &AudioBuffer { sample_rate: rate, channels: vec![vec![level; frames]; channels] },
        &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None },
    )
    .unwrap()
}

fn imported(bytes: &[u8]) -> (Engine, SourceId) {
    let mut e = Engine::default();
    let result = io::import_audio_bytes(&mut e, "take.wav", bytes, None, None, 12_000).unwrap();
    let id = SourceId(result["source"].as_u64().unwrap());
    let mut session = e.session().clone();
    session.sources[0].time_reference = 123_456;
    let clip = &mut session.tracks[0].playlists[0].clips[0];
    clip.content = soundcraft_model::ClipContent::Audio { source: id, offset: 7 };
    clip.length -= 10;
    let mut alternate = soundcraft_model::Playlist::new("Alternate");
    alternate.clips.push(Clip::audio(session.new_clip_id(), "Alternate", id, 4, 24_000, 8));
    session.tracks[0].playlists.push(alternate);
    (Engine::new(session), id)
}

fn missing(mut e: Engine, id: SourceId, legacy: bool) -> Engine {
    let mut session = e.session().clone();
    session.pool.remove(id);
    if legacy {
        session.sources.iter_mut().find(|source| source.id == id).unwrap().audio_identity = None;
    }
    e.replace_session(session);
    e
}

fn relink(e: &mut Engine, id: SourceId, path: &Path, allow: bool) -> soundcraft_engine::Result<serde_json::Value> {
    e.execute("media.relink", &json!({"source": id.0, "path": path.to_string_lossy(), "allow_unverified": allow}))
}

#[test]
fn identity_v1_has_a_stable_little_endian_vector_and_preserves_exact_sample_bits() {
    let mut buffer = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0, -0.0, 0.25]] };
    let original = media::audio_identity(&buffer);
    // Independent SHA-256 vector of the documented byte layout, including signed zero.
    assert_eq!(original, "pcm-sha256-v1:00b8d2643514c096483b045f4ab7929e2bf6c78d3fb69e05c4e001ec28db0b4e");
    buffer.channels[0][1] = 0.0;
    assert_ne!(media::audio_identity(&buffer), original);
    buffer.channels[0][1] = -0.0;
    buffer.channels[0][2] = f32::from_bits(0.25f32.to_bits() + 1);
    assert_ne!(media::audio_identity(&buffer), original, "no quantization may collapse distinct PCM");
    let chunked = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 1025]] };
    assert_eq!(media::audio_identity(&chunked), "pcm-sha256-v1:3ac01f65cad2e01686cc9c556944cab91a3c07769b4a2dda8906b74445c027cf");
}

#[test]
fn matching_audio_restores_all_takes_without_changing_clip_timing_and_is_undoable() {
    let f = Fixture::new();
    let bytes = wav(0.25, 2, 64, 48_000);
    let path = f.path("relocated.wav");
    std::fs::write(&path, &bytes).unwrap();
    let (e, id) = imported(&bytes);
    let mut e = missing(e, id, false);
    let before = e.session().clone();
    let r = relink(&mut e, id, &path, false).unwrap();
    assert_eq!(r["identity_verified"], true);
    assert!(e.session().pool.contains(id));
    assert_eq!(e.session().tracks, before.tracks);
    let original = &before.sources[0];
    let restored = &e.session().sources[0];
    assert_eq!(
        (restored.id, &restored.name, restored.channels, restored.frames, restored.sample_rate, restored.time_reference),
        (original.id, &original.name, original.channels, original.frames, original.sample_rate, original.time_reference)
    );
    assert!(Path::new(&restored.path).is_absolute());
    assert!(e.is_dirty());
    assert_eq!(e.undo_label(), Some("Relink Audio"));
    assert!(e.undo());
    assert_eq!(e.session(), &before);
    assert!(!e.session().pool.contains(id));
    assert!(e.redo());
    assert!(e.session().pool.contains(id));
    assert_eq!(e.session().tracks, before.tracks);
}

#[test]
fn same_name_and_metadata_cannot_override_known_content_identity_even_with_consent() {
    let f = Fixture::new();
    let (e, id) = imported(&wav(0.25, 1, 64, 48_000));
    let mut e = missing(e, id, false);
    let path = f.path("take.wav");
    std::fs::write(&path, wav(-0.25, 1, 64, 48_000)).unwrap();
    for allow in [false, true] {
        let before = e.session_arc();
        let revision = e.revision;
        assert!(relink(&mut e, id, &path, allow).unwrap_err().to_string().contains("identity differs"));
        assert_eq!(e.session(), before.as_ref());
        assert_eq!(e.revision, revision);
        assert!(!e.is_dirty());
        assert!(!e.can_undo());
    }
}

#[test]
fn legacy_sources_need_explicit_consent_but_still_reject_metadata_mismatches() {
    let f = Fixture::new();
    let bytes = wav(0.25, 1, 64, 48_000);
    let (e, id) = imported(&bytes);
    let mut e = missing(e, id, true);
    // The optional field is omitted, so old session JSON remains loadable without a format bump.
    let text = e.session().to_json().unwrap();
    assert!(!text.contains("audio_identity"));
    assert!(soundcraft_model::Session::from_json(&text).unwrap().sources[0].audio_identity.is_none());
    let path = f.path("take.wav");
    std::fs::write(&path, &bytes).unwrap();
    assert!(relink(&mut e, id, &path, false).unwrap_err().to_string().contains("legacy source"));
    for wrong in [wav(0.25, 2, 64, 48_000), wav(0.25, 1, 65, 48_000), wav(0.25, 1, 64, 96_000)] {
        std::fs::write(&path, wrong).unwrap();
        assert!(relink(&mut e, id, &path, true).unwrap_err().to_string().contains("metadata differs"));
        assert!(!e.session().pool.contains(id));
    }
    std::fs::write(&path, &bytes).unwrap();
    let r = relink(&mut e, id, &path, true).unwrap();
    assert_eq!(r["identity_verified"], false);
    assert_eq!(r["legacy_confirmation"], true);
    assert!(e.session().sources[0].audio_identity.is_some());
}

#[test]
fn saved_and_bundled_media_use_the_actual_encoded_pcm_identity_and_round_trip() {
    let f = Fixture::new();
    let (mut e, id) = imported(&wav(0.25, 1, 64, 48_000));
    // Existing Float32 WAV encoding clamps; save identity must describe the saved PCM.
    let mut session = e.session().clone();
    let buffer = AudioBuffer { sample_rate: 48_000, channels: vec![vec![1.25; 64]] };
    session.sources[0].audio_identity = Some(media::audio_identity(&buffer));
    session.pool.insert(id, std::sync::Arc::new(soundcraft_model::SourceAudio::new(buffer)));
    e.replace_session(session);
    let path = f.path("Session.scraft");
    io::save_session(&mut e, path.to_str().unwrap()).unwrap();
    let audio = f.0.join(&e.session().sources[0].path);
    let relocated = f.path("Original saved.wav");
    std::fs::rename(&audio, &relocated).unwrap();
    let mut opened = Engine::default();
    assert_eq!(io::open_session(&mut opened, path.to_str().unwrap()).unwrap().len(), 1);
    relink(&mut opened, id, &relocated, false).unwrap();
    assert_eq!(opened.session().pool.get(id).unwrap().buffer.channels[0][0], 1.0);
    let second = f.path("Second/Second.scraft");
    io::save_session(&mut opened, second.to_str().unwrap()).unwrap();
    let mut reopened = Engine::default();
    assert!(io::open_session(&mut reopened, second.to_str().unwrap()).unwrap().is_empty());
    assert_eq!(reopened.session().sources[0].audio_identity, opened.session().sources[0].audio_identity);
    assert_eq!(reopened.session().tracks, opened.session().tracks);
    let (bundle, saved) = io::session_bundle(reopened.session()).unwrap();
    let mut bundled = Engine::default();
    assert!(io::open_session_bytes(&mut bundled, "Bundle.scraftz", &bundle).unwrap().is_empty());
    assert_eq!(bundled.session().sources[0].audio_identity, saved.sources[0].audio_identity);
    assert!(bundled.session().pool.contains(id));
}

#[test]
fn session_open_rejects_changed_audio_instead_of_silently_loading_it() {
    let f = Fixture::new();
    let (mut e, id) = imported(&wav(0.25, 1, 64, 48_000));
    let path = f.path("Session.scraft");
    io::save_session(&mut e, path.to_str().unwrap()).unwrap();
    std::fs::write(f.0.join(&e.session().sources[0].path), wav(-0.25, 1, 64, 48_000)).unwrap();
    let mut opened = Engine::default();
    let unavailable = io::open_session(&mut opened, path.to_str().unwrap()).unwrap();
    assert!(unavailable[0].contains("identity differs"));
    assert!(!opened.session().pool.contains(id));
}

#[test]
fn autosave_does_not_reuse_stale_audio_after_a_confirmed_legacy_relink() {
    let f = Fixture::new();
    let (e, id) = imported(&wav(0.25, 1, 64, 48_000));
    let recovery = f.path("Recovery/Recovery.scraft");
    assert_eq!(io::save_copy(&e, recovery.to_str().unwrap()).unwrap(), 1);
    let mut e = missing(e, id, true);
    let replacement = f.path("confirmed-original.wav");
    std::fs::write(&replacement, wav(-0.25, 1, 64, 48_000)).unwrap();
    relink(&mut e, id, &replacement, true).unwrap();
    assert_eq!(io::save_copy(&e, recovery.to_str().unwrap()).unwrap(), 1);
    assert_eq!(io::save_copy(&e, recovery.to_str().unwrap()).unwrap(), 0);
    let mut reopened = Engine::default();
    assert!(io::open_session(&mut reopened, recovery.to_str().unwrap()).unwrap().is_empty());
    assert_eq!(reopened.session().pool.get(id).unwrap().buffer.channels[0][0], -0.25);
    let old_file = recovery.parent().unwrap().join(format!("Audio Files/take_{}.wav", id.0));
    let (_, old) = soundcraft_audio_io::decode(&std::fs::read(old_file).unwrap(), Some("wav")).unwrap();
    assert_eq!(old.channels[0][0], 0.25, "recovery must preserve the earlier recording");
}

#[test]
fn relink_uses_the_same_sample_rate_conversion_as_import() {
    let f = Fixture::new();
    let bytes = wav(0.1, 1, 441, 44_100);
    let path = f.path("take.wav");
    std::fs::write(&path, &bytes).unwrap();
    let (e, id) = imported(&bytes);
    let mut e = missing(e, id, false);
    assert_eq!(e.session().sources[0].sample_rate, 48_000);
    assert_eq!(relink(&mut e, id, &path, false).unwrap()["identity_verified"], true);
}

#[test]
fn importing_session_data_verifies_original_pcm_then_reidentifies_the_converted_source() {
    let f = Fixture::new();
    let mut original = Engine::new(soundcraft_model::Session::new("Original", soundcraft_time::SampleRate::HZ_44100));
    io::import_audio_bytes(&mut original, "take.wav", &wav(0.1, 1, 441, 44_100), None, None, 0).unwrap();
    let path = f.path("Original.scraft");
    io::save_session(&mut original, path.to_str().unwrap()).unwrap();
    let original_audio = f.0.join(&original.session().sources[0].path);
    let mut target = Engine::default();
    target.execute("file.import_session_data", &json!({"path": path})).unwrap();
    let converted = &target.session().sources[0];
    assert_eq!(converted.sample_rate, 48_000);
    assert_ne!(converted.audio_identity, original.session().sources[0].audio_identity);
    let id = converted.id;
    let expected = media::audio_identity(&target.session().pool.get(id).unwrap().buffer);
    assert_eq!(converted.audio_identity.as_deref(), Some(expected.as_str()));
    let mut target = missing(target, id, false);
    assert_eq!(relink(&mut target, id, &original_audio, false).unwrap()["identity_verified"], true);
    // Session data import must not quietly trust altered media from the original session.
    std::fs::write(&original_audio, wav(-0.1, 1, 441, 44_100)).unwrap();
    let mut rejected = Engine::default();
    let result = rejected.execute("file.import_session_data", &json!({"path": path})).unwrap();
    assert_eq!(result["missing"].as_array().unwrap().len(), 1);
    assert!(!rejected.session().pool.contains(rejected.session().sources[0].id));
}

#[test]
fn loaded_audio_and_invalid_candidates_leave_the_document_unchanged() {
    let f = Fixture::new();
    let bytes = wav(0.25, 1, 64, 48_000);
    let path = f.path("take.wav");
    std::fs::write(&path, &bytes).unwrap();
    let (mut e, id) = imported(&bytes);
    let before = e.session_arc();
    assert!(relink(&mut e, id, &path, true).unwrap_err().to_string().contains("already loaded"));
    assert_eq!(e.session(), before.as_ref());
    let mut e = missing(e, id, false);
    let corrupt = f.path("bad.wav");
    std::fs::write(&corrupt, b"not audio").unwrap();
    let oversized = f.path("too-large.wav");
    std::fs::File::create(&oversized).unwrap().set_len((1 << 30) + 1).unwrap();
    let excessive_conversion = f.path("tiny-rate.wav");
    std::fs::write(&excessive_conversion, wav(0.25, 1, 30_000, 1)).unwrap();
    for candidate in [&f.0, &f.path("absent.wav"), &corrupt, &oversized, &excessive_conversion] {
        let before = e.session_arc();
        assert!(relink(&mut e, id, candidate, false).is_err());
        assert_eq!(e.session(), before.as_ref());
        assert!(!e.can_undo());
    }
    for params in [
        json!({}),
        json!({"source": id.0, "path": ""}),
        json!({"source": id.0, "path": path, "allow_unverified": "yes"}),
        json!({"source": 999, "path": path}),
    ] {
        assert!(e.execute("media.relink", &params).is_err());
    }
    assert_eq!(e.execute("source.relink", &json!({"source": id.0, "path": path})).unwrap()["identity_verified"], true);
}

#[test]
fn unsupported_identity_versions_cannot_be_overridden() {
    let f = Fixture::new();
    let bytes = wav(0.25, 1, 64, 48_000);
    let path = f.path("take.wav");
    std::fs::write(&path, &bytes).unwrap();
    let (e, id) = imported(&bytes);
    let mut e = missing(e, id, false);
    let mut session = e.session().clone();
    session.sources[0].audio_identity = Some("future-identity-v2:unknown".into());
    e.replace_session(session);
    for allow in [false, true] {
        assert!(relink(&mut e, id, &path, allow).unwrap_err().to_string().contains("unsupported audio identity version"));
        assert!(!e.session().pool.contains(id));
        assert!(!e.can_undo());
    }
}

#[test]
fn shared_file_cache_checks_each_sources_identity_and_metadata() {
    let f = Fixture::new();
    let (mut e, original) = imported(&wav(0.25, 1, 64, 48_000));
    let path = f.path("Shared.scraft");
    io::save_session(&mut e, path.to_str().unwrap()).unwrap();
    let mut s = e.session().clone();
    let mut other = s.sources[0].clone();
    other.id = SourceId(s.alloc());
    let other_id = other.id;
    other.audio_identity = Some(media::audio_identity(&AudioBuffer { sample_rate: 48_000, channels: vec![vec![-0.25; 64]] }));
    s.sources.push(other);
    std::fs::write(&path, s.to_json().unwrap()).unwrap();
    let mut opened = Engine::default();
    let missing = io::open_session(&mut opened, path.to_str().unwrap()).unwrap();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].contains("identity differs"));
    assert!(opened.session().pool.contains(original));
    assert!(!opened.session().pool.contains(other_id));
    s.sources[1].audio_identity = s.sources[0].audio_identity.clone();
    s.sources[1].frames += 1;
    std::fs::write(&path, s.to_json().unwrap()).unwrap();
    let missing = io::open_session(&mut opened, path.to_str().unwrap()).unwrap();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].contains("metadata differs"));
    assert!(opened.session().pool.contains(original));
    assert!(!opened.session().pool.contains(other_id));
}

#[test]
fn shared_donor_audio_remains_shared_after_verified_rate_conversion() {
    let f = Fixture::new();
    let mut original = Engine::new(soundcraft_model::Session::new("Original", soundcraft_time::SampleRate::HZ_44100));
    io::import_audio_bytes(&mut original, "take.wav", &wav(0.1, 1, 441, 44_100), None, None, 0).unwrap();
    let mut s = original.session().clone();
    let mut src = s.sources[0].clone();
    let first_id = src.id;
    src.id = SourceId(s.alloc());
    let second_id = src.id;
    let audio = s.pool.get(first_id).unwrap().clone();
    s.pool.insert(second_id, audio);
    s.sources.push(src);
    let mut track = s.tracks[0].clone();
    track.id = soundcraft_model::TrackId(s.alloc());
    track.name = "Shared donor".into();
    for playlist in &mut track.playlists {
        for clip in &mut playlist.clips {
            clip.id = s.new_clip_id();
            if let soundcraft_model::ClipContent::Audio { source, .. } = &mut clip.content {
                *source = second_id;
            }
        }
    }
    s.tracks.push(track);
    original.replace_session(s);
    let path = f.path("Shared donor.scraft");
    io::save_session(&mut original, path.to_str().unwrap()).unwrap();
    assert_eq!(original.session().sources[0].path, original.session().sources[1].path);
    let mut target = Engine::default();
    target.execute("file.import_session_data", &json!({"path": path})).unwrap();
    let sources = &target.session().sources;
    assert_eq!(sources.len(), 2);
    let first = target.session().pool.get(sources[0].id).unwrap();
    let second = target.session().pool.get(sources[1].id).unwrap();
    assert!(std::sync::Arc::ptr_eq(first, second));
    assert_eq!(first.buffer.sample_rate, 48_000);
    let identity = media::audio_identity(&first.buffer);
    assert!(sources.iter().all(|src| src.audio_identity.as_deref() == Some(identity.as_str())));
}

#[test]
fn background_autosave_recovers_confirmed_relink_and_keeps_main_copy_cleanup() {
    let f = Fixture::new();
    let root = f.path("Autosaves");
    let writer = soundcraft_engine::autosave::Autosaver::new(root.clone());
    let (e, id) = imported(&wav(0.25, 1, 64, 48_000));
    writer.write(e.session_arc(), None, None);
    writer.flush().unwrap();
    let dir = std::fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
    let path = dir.join("session.scraft");
    let before = soundcraft_model::Session::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let old_file = PathBuf::from(&before.sources[0].path);
    assert!(old_file.is_file());
    let mut e = missing(e, id, true);
    let replacement = f.path("confirmed-original.wav");
    let bytes = wav(-0.25, 1, 64, 48_000);
    std::fs::write(&replacement, &bytes).unwrap();
    relink(&mut e, id, &replacement, true).unwrap();
    writer.write(e.session_arc(), None, None);
    writer.flush().unwrap();
    let mut recovered = Engine::default();
    assert!(io::open_session(&mut recovered, path.to_str().unwrap()).unwrap().is_empty());
    assert_eq!(recovered.session().pool.get(id).unwrap().buffer.channels[0][0], -0.25);
    assert_eq!(recovered.session().sources[0].audio_identity, e.session().sources[0].audio_identity);
    assert_eq!(recovered.session().tracks, e.session().tracks);
    assert!(!old_file.exists(), "main removes obsolete private copies");
    assert_eq!(std::fs::read(&replacement).unwrap(), bytes, "relink's selected file stays unchanged");
}

#[test]
fn background_autosave_identity_describes_encoded_pcm_on_new_and_reused_copies() {
    let f = Fixture::new();
    let root = f.path("Autosaves");
    let writer = soundcraft_engine::autosave::Autosaver::new(root.clone());
    let mut e = Engine::default();
    let id = io::add_source(
        e.session_mut(),
        "nonfinite",
        AudioBuffer { sample_rate: 48_000, channels: vec![vec![f32::NAN, f32::INFINITY, 1.25]] },
        None,
        FileFormat::Wav,
    );
    for _ in 0..2 {
        writer.write(e.session_arc(), None, None);
        writer.flush().unwrap();
        let dir = std::fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
        let mut recovered = Engine::default();
        assert!(io::open_session(&mut recovered, dir.join("session.scraft").to_str().unwrap()).unwrap().is_empty());
        let audio = &recovered.session().pool.get(id).unwrap().buffer;
        assert_eq!(audio.channels[0], vec![0.0, 1.0, 1.0]);
        let identity = media::audio_identity(audio);
        assert_eq!(recovered.session().sources[0].audio_identity.as_deref(), Some(identity.as_str()));
    }
}

#[test]
fn relink_respects_automation_read_roots_without_mutating_rejected_sessions() {
    let f = Fixture::new();
    let allowed = f.path("Allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    let bytes = wav(0.25, 1, 64, 48_000);
    let outside = f.path("outside.wav");
    let inside = allowed.join("inside.wav");
    std::fs::write(&outside, &bytes).unwrap();
    std::fs::write(&inside, &bytes).unwrap();
    let (e, id) = imported(&bytes);
    let mut e = missing(e, id, false);
    let before = e.session_arc();
    let roots = std::sync::Arc::new(soundcraft_engine::file_access::AutomationRoots::new(Some(&allowed), None).unwrap().unwrap());
    let error = soundcraft_engine::file_access::confine(Some(&roots), || relink(&mut e, id, &outside, false)).unwrap_err();
    assert!(error.to_string().contains("outside the read root"));
    assert_eq!(e.session(), before.as_ref());
    assert!(!e.can_undo());
    assert!(!e.is_dirty());
    let result = soundcraft_engine::file_access::confine(Some(&roots), || relink(&mut e, id, &inside, false)).unwrap();
    assert_eq!(result["identity_verified"], true);
    assert!(e.session().pool.contains(id));
}

#[test]
fn shared_saved_audio_receives_the_encoded_identity_for_every_source() {
    let f = Fixture::new();
    let mut e = Engine::default();
    let first =
        io::add_source(e.session_mut(), "shared", AudioBuffer { sample_rate: 48_000, channels: vec![vec![f32::NAN, 1.25]] }, None, FileFormat::Wav);
    let s = e.session_mut();
    let mut other = s.sources[0].clone();
    other.id = SourceId(s.alloc());
    let second = other.id;
    let audio = s.pool.get(first).unwrap().clone();
    s.pool.insert(second, audio);
    s.sources.push(other);
    let path = f.path("Shared.scraft");
    assert_eq!(io::save_session(&mut e, path.to_str().unwrap()).unwrap(), 1);
    let mut reopened = Engine::default();
    assert!(io::open_session(&mut reopened, path.to_str().unwrap()).unwrap().is_empty());
    assert_eq!(reopened.session().sources[0].path, reopened.session().sources[1].path);
    assert_eq!(reopened.session().sources[0].audio_identity, reopened.session().sources[1].audio_identity);
    let first = reopened.session().pool.get(first).unwrap();
    let second = reopened.session().pool.get(second).unwrap();
    assert!(std::sync::Arc::ptr_eq(first, second));
    assert_eq!(first.buffer.channels[0], vec![0.0, 1.0]);
}
