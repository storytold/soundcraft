//! Native encoded originals stay separate from decoded playback audio and follow document leases.
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat, SampleFormat};
use soundcraft_engine::{Engine, EngineError, io};
use soundcraft_model::{Clip, Session, SourceId};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const PCM: [i16; 16] = [0, -32768, 32767, 8192, -16384, 12345, -1, 1, 24576, -8192, 16384, -24576, 42, -42, 30000, -30000];
// Independently calculated from the literal RIFF layout below, rather than the originals store.
const ORIGINAL_SHA256: &str = "ee6b63f908392b29073d7b680a7c9a6cbaef3631432359a5886c57516e3518dd";
const ORIGINAL_BYTES: u64 = 744;

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("soundcraft-encoded-originals-{}-{suffix}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => panic!("create {}: {err}", path.display()),
            }
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn chunk(bytes: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    bytes.extend_from_slice(tag);
    bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(payload);
    if payload.len() & 1 != 0 {
        bytes.push(0);
    }
}

/// Handmade PCM16 WAVE: BWF fields, an odd-sized unknown chunk and a post-data opaque chunk.
/// No project encoder or decoder constructs the expected original file.
fn encoded_fixture() -> Vec<u8> {
    let mut body = b"WAVE".to_vec();
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
    fmt.extend_from_slice(&48_000u32.to_le_bytes());
    fmt.extend_from_slice(&96_000u32.to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    chunk(&mut body, b"fmt ", &fmt);
    let mut bext = vec![0u8; 602];
    for (at, value) in [
        (0, b"Independent source-byte fixture".as_slice()),
        (256, b"SoundCraft tests".as_slice()),
        (288, b"fixture-original-001".as_slice()),
        (320, b"2026-10-14".as_slice()),
        (330, b"12:00:00".as_slice()),
    ] {
        bext[at..at + value.len()].copy_from_slice(value);
    }
    bext[338..346].copy_from_slice(&1234u64.to_le_bytes());
    chunk(&mut body, b"bext", &bext);
    chunk(&mut body, b"qXyz", &[128, 0, 254, 1, 127]);
    let data: Vec<u8> = PCM.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    chunk(&mut body, b"data", &data);
    chunk(&mut body, b"JUNK", b"after audio, keep these exact bytes");
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&body);
    assert_eq!(bytes.len() as u64, ORIGINAL_BYTES);
    bytes
}

fn expected_pcm() -> Vec<f32> {
    PCM.iter().map(|sample| f32::from(*sample) / 32768.0).collect()
}

fn import(e: &mut Engine, input: &Path, at: i64) -> SourceId {
    let result = e.execute("file.import_audio", &json!({"path": input, "at": at})).unwrap();
    SourceId(result["imported"][0]["source"].as_u64().unwrap())
}

fn imported(dir: &FixtureDir) -> (Engine, SourceId, PathBuf, Vec<u8>) {
    let input = dir.path("input.wav");
    let bytes = encoded_fixture();
    std::fs::write(&input, &bytes).unwrap();
    let mut e = Engine::default();
    let id = import(&mut e, &input, 37);
    (e, id, input, bytes)
}

fn assert_decoded(e: &Engine, id: SourceId) {
    let source = e.session().source(id).unwrap();
    assert_eq!(source.channels, 1);
    assert_eq!(source.frames, PCM.len() as u64);
    assert_eq!(source.sample_rate, 48_000);
    assert_eq!(source.time_reference, 1234);
    let audio = &e.session().pool.get(id).unwrap().buffer;
    assert_eq!(audio.sample_rate, 48_000);
    assert_eq!(audio.channels, vec![expected_pcm()]);
    let clip = &e.session().tracks[0].clips()[0];
    assert_eq!(clip.source(), Some(id));
    assert_eq!(clip.start, 37);
    assert_eq!(clip.length, PCM.len() as i64);
}

fn assert_provenance(e: &Engine, id: SourceId, raw: &[u8]) {
    let original = e.session().originals.get(&id).unwrap();
    assert_eq!(original.version, 1);
    assert_eq!(original.sha256, ORIGINAL_SHA256);
    assert_eq!(original.bytes, ORIGINAL_BYTES);
    assert_eq!(original.path, format!("Original Files/{ORIGINAL_SHA256}.asset"));
    let asset = e.session().pool.original_asset(id).unwrap();
    assert_eq!(std::fs::read(&asset.path).unwrap(), raw);
}

fn save(e: &mut Engine, path: &Path) {
    e.execute("session.save_as", &json!({"path": path})).unwrap();
}

fn open(path: &Path) -> Engine {
    let mut e = Engine::default();
    let result = e.execute("session.open", &json!({"path": path})).unwrap();
    assert!(result["missing"].as_array().unwrap().is_empty(), "{result}");
    e
}

fn managed_original(e: &Engine, id: SourceId, folder: &Path) -> PathBuf {
    folder.join(&e.session().originals.get(&id).unwrap().path)
}

fn wait_removed(path: &Path) {
    let started = Instant::now();
    while std::fs::symlink_metadata(path).is_ok() && started.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(std::fs::symlink_metadata(path).is_err(), "expired temporary original still exists: {}", path.display());
}

fn assert_missing_original(e: &mut Engine, id: SourceId, path: &Path, copy: &Path) {
    let result = e.execute("session.open", &json!({"path": path})).unwrap();
    assert!(!result["missing"].as_array().unwrap().is_empty(), "{result}");
    assert!(e.messages.iter().any(|message| message.contains("missing media")), "{:?}", e.messages);
    assert!(e.session().originals.contains_key(&id), "missing-media reporting erased provenance");
    assert!(e.session().pool.original_asset(id).is_none());
    assert_decoded(e, id);
    let before = e.session_arc();
    let before_path = e.path.clone();
    let history = e.undo_history();
    let dirty = e.is_dirty();
    let document_bytes = std::fs::read(path).unwrap();
    let err = e.execute("session.save", &json!({})).unwrap_err();
    assert!(err.to_string().to_ascii_lowercase().contains("original"), "{err}");
    let err = e.execute("session.save_copy", &json!({"path": copy})).unwrap_err();
    assert!(err.to_string().to_ascii_lowercase().contains("original"), "{err}");
    assert_eq!(e.session(), before.as_ref());
    assert_eq!(e.path, before_path);
    assert_eq!(e.undo_history(), history);
    assert_eq!(e.is_dirty(), dirty);
    assert_eq!(std::fs::read(path).unwrap(), document_bytes);
    assert!(!copy.exists(), "failed save published a session without its originals");
}

#[test]
fn native_import_save_and_reopen_preserve_opaque_encoded_bytes_and_derived_float32_audio() {
    let dir = FixtureDir::new();
    let (mut e, id, input, raw) = imported(&dir);
    assert_decoded(&e, id);
    assert_provenance(&e, id, &raw);
    assert_ne!(e.session().pool.original_asset(id).unwrap().path, input);
    let path = dir.path("project/Project.scraft");
    save(&mut e, &path);
    let folder = path.parent().unwrap();
    assert_eq!(std::fs::read(managed_original(&e, id, folder)).unwrap(), raw);
    let source = e.session().source(id).unwrap();
    let decoded_bytes = std::fs::read(folder.join(&source.path)).unwrap();
    assert_ne!(decoded_bytes, raw, "the playback WAV was substituted for the encoded original");
    let (info, audio) = soundcraft_audio_io::decode(&decoded_bytes, Some("wav")).unwrap();
    assert_eq!(info.sample_format, SampleFormat::Float32);
    assert_eq!(audio.channels, vec![expected_pcm()]);
    assert!(info.bwf.is_none(), "derived media remains the existing plain float WAV");
    let reopened = open(&path);
    assert_decoded(&reopened, id);
    assert_provenance(&reopened, id, &raw);
    assert_eq!(std::fs::read(input).unwrap(), raw);
}

#[test]
fn changing_or_deleting_the_external_file_after_import_does_not_damage_the_capture() {
    for delete in [false, true] {
        let dir = FixtureDir::new();
        let (mut e, id, input, raw) = imported(&dir);
        if delete {
            std::fs::remove_file(&input).unwrap();
        } else {
            std::fs::write(&input, b"external file changed after import").unwrap();
        }
        assert_decoded(&e, id);
        assert_provenance(&e, id, &raw);
        let path = dir.path("project/Project.scraft");
        save(&mut e, &path);
        let reopened = open(&path);
        assert_decoded(&reopened, id);
        assert_provenance(&reopened, id, &raw);
        drop(reopened);
        drop(e);
        if delete {
            assert!(!input.exists());
        } else {
            assert_eq!(std::fs::read(input).unwrap(), b"external file changed after import");
        }
    }
}

#[test]
fn save_copy_keeps_originals_without_retargeting_the_open_document_or_its_history() {
    let dir = FixtureDir::new();
    let (mut e, id, _, raw) = imported(&dir);
    let path = dir.path("project/Project.scraft");
    save(&mut e, &path);
    e.execute("track.new", &json!({"name": "Extra A", "format": "mono"})).unwrap();
    e.execute("track.new", &json!({"name": "Extra B", "format": "mono"})).unwrap();
    assert!(e.undo());
    assert!(e.is_dirty());
    assert!(e.can_redo());
    let before = e.session_arc();
    let before_path = e.path.clone();
    let revision = e.revision;
    let history = e.undo_history();
    let copy = dir.path("copy/Copy.scraft");
    e.execute("session.save_copy", &json!({"path": copy})).unwrap();
    assert_eq!(e.session(), before.as_ref());
    assert_eq!(e.path, before_path);
    assert_eq!(e.revision, revision);
    assert_eq!(e.undo_history(), history);
    assert!(e.is_dirty());
    assert!(e.can_redo());
    assert_provenance(&e, id, &raw);
    let copied = open(&copy);
    assert_decoded(&copied, id);
    assert_provenance(&copied, id, &raw);
    assert_eq!(std::fs::read(managed_original(&copied, id, copy.parent().unwrap())).unwrap(), raw);
    assert!(copied.session().track_by_name("Extra A").is_some());
    assert!(copied.session().track_by_name("Extra B").is_none());
    let original = open(&path);
    assert!(original.session().track_by_name("Extra A").is_none());
    assert_provenance(&original, id, &raw);
    assert!(e.redo());
    assert!(e.session().track_by_name("Extra B").is_some());
}

#[test]
fn undo_redo_and_session_snapshots_keep_temporary_originals_until_the_last_lease_drops() {
    let dir = FixtureDir::new();
    let (mut e, id, input, raw) = imported(&dir);
    let path = e.session().pool.original_asset(id).unwrap().path.clone();
    let snapshot = e.session_arc();
    assert!(e.undo());
    assert!(e.session().originals.is_empty());
    assert!(e.session().pool.original_asset(id).is_none());
    assert_eq!(std::fs::read(&path).unwrap(), raw, "Redo must retain the imported lease");
    assert!(e.redo());
    assert_provenance(&e, id, &raw);
    assert!(e.undo());
    e.execute("session.new", &json!({"name": "Replacement"})).unwrap();
    assert!(!e.can_undo());
    assert!(!e.can_redo());
    assert_eq!(std::fs::read(&path).unwrap(), raw, "the external snapshot still owns the lease");
    drop(snapshot);
    wait_removed(&path);
    assert_eq!(std::fs::read(input).unwrap(), raw, "cleanup must not remove the imported external file");
}

#[test]
fn dropping_an_engine_collects_unsaved_temporary_originals_without_an_explicit_collection_call() {
    let dir = FixtureDir::new();
    let (e, id, input, raw) = imported(&dir);
    let path = e.session().pool.original_asset(id).unwrap().path.clone();
    assert_eq!(std::fs::read(&path).unwrap(), raw);
    drop(e);
    wait_removed(&path);
    assert_eq!(std::fs::read(input).unwrap(), raw);
}

#[test]
fn missing_or_same_length_corrupt_originals_keep_provenance_and_refuse_incomplete_saves() {
    for corrupt in [false, true] {
        let dir = FixtureDir::new();
        let (mut e, id, _, raw) = imported(&dir);
        let path = dir.path("project/Project.scraft");
        save(&mut e, &path);
        let original = managed_original(&e, id, path.parent().unwrap());
        if corrupt {
            let mut changed = raw;
            // An opaque byte changes while the size and playable PCM remain identical.
            changed[656] ^= 0x20;
            std::fs::write(&original, changed).unwrap();
        } else {
            std::fs::remove_file(&original).unwrap();
        }
        let mut reopened = Engine::default();
        assert_missing_original(&mut reopened, id, &path, &dir.path("copy/Copy.scraft"));
        assert_eq!(reopened.session().originals.get(&id), e.session().originals.get(&id));
    }
}

#[test]
fn traversal_and_absolute_original_paths_are_not_loaded_copied_or_deleted() {
    for absolute in [false, true] {
        let dir = FixtureDir::new();
        let (mut e, id, _, raw) = imported(&dir);
        let path = dir.path("project/Project.scraft");
        save(&mut e, &path);
        let outside = dir.path("outside.asset");
        std::fs::write(&outside, &raw).unwrap();
        let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let metadata = document["originals"].as_object_mut().unwrap().values_mut().next().unwrap();
        metadata["path"] = json!(if absolute { outside.to_string_lossy().into_owned() } else { "../outside.asset".into() });
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let mut reopened = Engine::default();
        assert_missing_original(&mut reopened, id, &path, &dir.path("copy/Copy.scraft"));
        reopened.execute("session.new", &json!({})).unwrap();
        drop(reopened);
        assert_eq!(std::fs::read(outside).unwrap(), raw);
    }
}

#[cfg(unix)]
#[test]
fn symlinked_original_files_and_directories_are_not_loaded_copied_or_deleted() {
    for linked_directory in [false, true] {
        let dir = FixtureDir::new();
        let (mut e, id, _, raw) = imported(&dir);
        let path = dir.path("project/Project.scraft");
        save(&mut e, &path);
        let original = managed_original(&e, id, path.parent().unwrap());
        let outside_dir = dir.path("outside");
        std::fs::create_dir(&outside_dir).unwrap();
        let outside = outside_dir.join(original.file_name().unwrap());
        std::fs::write(&outside, &raw).unwrap();
        if linked_directory {
            std::fs::remove_dir_all(original.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&outside_dir, original.parent().unwrap()).unwrap();
        } else {
            std::fs::remove_file(&original).unwrap();
            std::os::unix::fs::symlink(&outside, &original).unwrap();
        }
        let mut reopened = Engine::default();
        assert_missing_original(&mut reopened, id, &path, &dir.path("copy/Copy.scraft"));
        reopened.execute("session.close", &json!({})).unwrap();
        drop(reopened);
        drop(e);
        assert_eq!(std::fs::read(outside).unwrap(), raw);
        assert!(std::fs::symlink_metadata(if linked_directory { original.parent().unwrap() } else { &original }).unwrap().file_type().is_symlink());
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_original_destination_cannot_redirect_a_save_outside_the_session() {
    let dir = FixtureDir::new();
    let (mut e, id, _, raw) = imported(&dir);
    let folder = dir.path("project");
    let outside = dir.path("outside");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    std::fs::write(&sentinel, b"do not replace or delete").unwrap();
    std::os::unix::fs::symlink(&outside, folder.join("Original Files")).unwrap();
    let before = e.session_arc();
    let before_path = e.path.clone();
    let history = e.undo_history();
    let dirty = e.is_dirty();
    let path = folder.join("Project.scraft");
    let err = e.execute("session.save_as", &json!({"path": path})).unwrap_err();
    assert!(err.to_string().to_ascii_lowercase().contains("original"), "{err}");
    assert_eq!(e.session(), before.as_ref());
    assert_eq!(e.path, before_path);
    assert_eq!(e.undo_history(), history);
    assert_eq!(e.is_dirty(), dirty);
    assert_provenance(&e, id, &raw);
    assert!(!path.exists());
    assert_eq!(std::fs::read(sentinel).unwrap(), b"do not replace or delete");
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 1, "the save followed the directory link and created an outside asset");
}

#[test]
fn bundles_with_originals_are_explicitly_unsupported_and_leave_the_document_alone() {
    use std::io::Write as _;
    let dir = FixtureDir::new();
    let (mut e, id, _, raw) = imported(&dir);
    let before = e.session_arc();
    let history = e.undo_history();
    let before_path = e.path.clone();
    let dirty = e.is_dirty();
    let err = io::session_bundle(e.session()).unwrap_err();
    assert!(matches!(err, EngineError::Unsupported(_)), "{err}");
    assert!(err.to_string().to_ascii_lowercase().contains("original"), "{err}");
    let text = e.session().to_json().unwrap();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("Fixture.scraft", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(text.as_bytes()).unwrap();
    let bundle = zip.finish().unwrap().into_inner();
    for bytes in [text.as_bytes(), bundle.as_slice()] {
        let err = io::open_session_bytes(&mut e, "Fixture.scraftz", bytes).unwrap_err();
        assert!(matches!(err, EngineError::Unsupported(_)), "{err}");
        assert!(err.to_string().to_ascii_lowercase().contains("original"), "{err}");
        assert_eq!(e.session(), before.as_ref());
        assert_eq!(e.path, before_path);
        assert_eq!(e.undo_history(), history);
        assert_eq!(e.is_dirty(), dirty);
        assert_provenance(&e, id, &raw);
    }
}

#[test]
fn legacy_native_json_without_an_originals_manifest_loads_unchanged() {
    let dir = FixtureDir::new();
    let mut e = Engine::default();
    e.execute("track.new", &json!({"name": "Legacy", "format": "mono"})).unwrap();
    let track = e.session().tracks[0].id;
    let audio = AudioBuffer { sample_rate: 48_000, channels: vec![expected_pcm()] };
    let id = io::add_source(e.session_mut(), "legacy", audio, None, FileFormat::Wav);
    let clip_id = e.session_mut().new_clip_id();
    soundcraft_engine::edit::place_clip(e.session_mut(), track, Clip::audio(clip_id, "legacy", id, 0, 37, PCM.len() as i64));
    e.session_mut().sources[0].time_reference = 1234;
    let path = dir.path("project/Legacy.scraft");
    save(&mut e, &path);
    let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document.as_object_mut().unwrap().remove("originals");
    let text = serde_json::to_string(&document).unwrap();
    let model = Session::from_json(&text).unwrap();
    assert!(model.originals.is_empty());
    std::fs::write(&path, text).unwrap();
    let reopened = open(&path);
    assert_decoded(&reopened, id);
    assert!(reopened.session().originals.is_empty());
    assert!(reopened.session().pool.original_asset(id).is_none());
    assert_eq!(reopened.session().sources, e.session().sources);
    assert_eq!(reopened.session().tracks, e.session().tracks);
    assert!(io::session_bundle(reopened.session()).is_ok(), "existing decoded-only bundle support remains available");
}

#[test]
fn confined_import_can_publish_private_staging_without_granting_other_filesystem_access() {
    use soundcraft_engine::file_access::{self, AutomationRoots};
    let dir = FixtureDir::new();
    let allowed = dir.path("allowed");
    let project = allowed.join("project");
    let outside = dir.path("outside");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let input = allowed.join("input.wav");
    let excluded_input = outside.join("input.wav");
    let raw = encoded_fixture();
    std::fs::write(&input, &raw).unwrap();
    std::fs::write(&excluded_input, &raw).unwrap();
    let roots = AutomationRoots::new(Some(&allowed), Some(&project)).unwrap().unwrap();
    file_access::confine(Some(&roots), || {
        let mut e = Engine::default();
        let id = import(&mut e, &input, 37);
        let staged = e.session().pool.original_asset(id).unwrap().path.clone();
        assert!(!staged.starts_with(&allowed), "the fixture must exercise app-owned scratch outside the read root");
        assert!(io::read_file(&staged).is_err(), "private staging must not broaden command read authority");
        let path = project.join("Project.scraft");
        save(&mut e, &path);
        assert_provenance(&e, id, &raw);
        assert_eq!(std::fs::read(managed_original(&e, id, &project)).unwrap(), raw);
        let reopened = open(&path);
        assert_decoded(&reopened, id);
        assert_provenance(&reopened, id, &raw);
        let before = e.session_arc();
        let before_path = e.path.clone();
        let history = e.undo_history();
        let err = e.execute("file.import_audio", &json!({"path": excluded_input})).unwrap_err();
        assert!(err.to_string().contains("outside the read root"), "{err}");
        let excluded_copy = outside.join("Copy.scraft");
        let err = e.execute("session.save_copy", &json!({"path": excluded_copy})).unwrap_err();
        assert!(err.to_string().contains("outside the write root"), "{err}");
        assert_eq!(e.session(), before.as_ref());
        assert_eq!(e.path, before_path);
        assert_eq!(e.undo_history(), history);
        assert!(!e.is_dirty());
        assert!(!excluded_copy.exists());
        assert_eq!(std::fs::read(&excluded_input).unwrap(), raw);
    });
}
