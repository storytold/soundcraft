//! Offline delivery must not occupy a current document's audio or video locator.
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::file_access::{AutomationRoots, confine};
use soundcraft_engine::io::{Bounce, add_recording, add_source, import_audio_bytes};
use soundcraft_model::{ChannelFormat, SourceId, TrackKind, VideoSource};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Folder(PathBuf);

impl Folder {
    fn new() -> Self {
        Self::under(&std::env::temp_dir())
    }

    fn under(parent: &Path) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path = parent.join(format!("soundcraft-media-locators-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("create owned fixture: {err}"),
            }
        }
        panic!("no unused fixture directory");
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options() -> EncodeOptions {
    EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None }
}

fn audio(rate: u32) -> AudioBuffer {
    AudioBuffer { sample_rate: rate, channels: vec![(0..8192).map(|i| (i as f32 * 0.03).sin() * 0.2).collect()] }
}

fn imported(path: &Path, rate: u32) -> Engine {
    let mut e = Engine::default();
    let bytes = soundcraft_audio_io::encode(&audio(rate), &options()).unwrap();
    import_audio_bytes(&mut e, "take.wav", &bytes, Some(path.to_str().unwrap()), None, 0).unwrap();
    e
}

fn bounce(e: &Engine, path: &Path) -> soundcraft_engine::Result<Bounce> {
    Bounce::new(e.session_arc(), path, Range::new(0, 4096), &options(), None, false, false)
}

fn refused(e: &Engine, path: &Path) -> String {
    assert!(!path.exists(), "a reservation test starts with an absent destination");
    let mut before: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    before.sort();
    let error = bounce(e, path).err().expect("reserved locator must be refused").to_string();
    assert!(!path.exists());
    let mut after: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    after.sort();
    assert_eq!(before, after, "refusal must not leave a staging file");
    error
}

fn video(e: &mut Engine, path: &str) {
    e.session_mut().videos.push(VideoSource {
        id: SourceId(900_000),
        name: "picture".into(),
        path: path.into(),
        width: 1920,
        height: 1080,
        frame_rate: 24.0,
        duration: 1.0,
        codec: "h264".into(),
    });
}

#[test]
fn missing_unsaved_import_locator_is_reserved() {
    let f = Folder::new();
    let path = f.join("import.wav");
    let e = imported(&path, 48_000);
    assert!(e.session().sources[0].unsaved);
    refused(&e, &path);
}

#[test]
fn resampled_import_keeps_its_original_locator_reserved() {
    let f = Folder::new();
    let path = f.join("resampled.wav");
    let e = imported(&path, 44_100);
    assert_eq!(e.session().sources[0].sample_rate, e.session().sample_rate.hz());
    refused(&e, &path);
}

#[test]
fn generated_unreferenced_source_reserves_its_future_session_file() {
    let f = Folder::new();
    std::fs::create_dir(f.join("Audio Files")).unwrap();
    let mut e = Engine::default();
    e.session_mut().folder = Some(f.0.clone());
    add_source(e.session_mut(), "generated", audio(48_000), None, FileFormat::Wav);
    assert!(e.session().tracks.is_empty());
    let path = f.0.join(&e.session().sources[0].path);
    refused(&e, &path);
}

#[test]
fn decoded_recording_reserves_its_missing_take_locator() {
    let f = Folder::new();
    let path = f.join("recording.wav");
    std::fs::write(&path, soundcraft_audio_io::encode(&audio(48_000), &options()).unwrap()).unwrap();
    let mut e = Engine::default();
    let track = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Recorded"));
    assert_eq!(add_recording(&mut e, 0, vec![(track, path.clone())], None).len(), 1);
    std::fs::remove_file(&path).unwrap();
    refused(&e, &path);
}

#[test]
fn audio_relative_locator_uses_session_folder() {
    let f = Folder::new();
    let mut e = imported(&f.join("unused.wav"), 48_000);
    e.session_mut().folder = Some(f.0.clone());
    e.session_mut().sources[0].path = "relative.wav".into();
    refused(&e, &f.join("relative.wav"));
}

#[test]
fn moved_session_reserves_relative_locator_in_its_current_folder() {
    let old = Folder::new();
    let moved = Folder::new();
    let mut e = imported(&old.join("unused.wav"), 48_000);
    e.session_mut().folder = Some(old.0.clone());
    e.session_mut().sources[0].path = "take.wav".into();
    e.session_mut().folder = Some(moved.0.clone());
    refused(&e, &moved.join("take.wav"));
    bounce(&e, &old.join("take.wav")).unwrap().run().unwrap();
}

#[test]
fn relative_locator_without_session_folder_uses_current_directory() {
    let cwd = std::env::current_dir().unwrap();
    let f = Folder::new();
    let name = format!("{}-audio.wav", f.0.file_name().unwrap().to_str().unwrap());
    let mut e = imported(&f.join("unused.wav"), 48_000);
    e.session_mut().sources[0].path = name.clone();
    assert!(e.session().folder.is_none());
    let error = refused(&e, &cwd.join(name));
    assert!(error.contains("current media locator"), "{error}");
}

#[test]
fn absolute_video_locator_is_reserved_without_a_video_clip() {
    let f = Folder::new();
    let path = f.join("picture.mp4");
    let mut e = Engine::default();
    video(&mut e, path.to_str().unwrap());
    refused(&e, &path);
}

#[test]
fn relative_video_locator_uses_session_folder() {
    let f = Folder::new();
    let mut e = Engine::default();
    e.session_mut().folder = Some(f.0.clone());
    video(&mut e, "picture.mp4");
    refused(&e, &f.join("picture.mp4"));
}

#[test]
fn relative_video_without_session_folder_uses_current_directory() {
    let cwd = std::env::current_dir().unwrap();
    let f = Folder::new();
    let name = format!("{}-video.mp4", f.0.file_name().unwrap().to_str().unwrap());
    let mut e = Engine::default();
    video(&mut e, &name);
    let error = refused(&e, &cwd.join(name));
    assert!(error.contains("current media locator"), "{error}");
}

#[test]
fn existing_ancestor_traversal_resolves_to_reserved_audio_name() {
    let f = Folder::new();
    std::fs::create_dir(f.join("sub")).unwrap();
    let path = f.join("take.wav");
    let mut e = imported(&path, 48_000);
    e.session_mut().folder = Some(f.0.clone());
    e.session_mut().sources[0].path = "sub/../take.wav".into();
    refused(&e, &path);
}

#[test]
fn unresolved_traversal_fails_closed_without_disclosing_locator() {
    let f = Folder::new();
    let mut e = imported(&f.join("unused.wav"), 48_000);
    e.session_mut().folder = Some(f.0.clone());
    // Windows resolves parent traversal lexically, so use its reserved-device form there.
    let secret = if cfg!(windows) { "private-missing-directory/NUL.wav" } else { "private-missing-directory/../private-source.wav" };
    e.session_mut().sources[0].path = secret.into();
    let error = refused(&e, &f.join("unrelated.wav"));
    assert!(error.contains("bounce media locator name cannot be validated"), "{error}");
    assert!(!error.contains("private-missing-directory") && !error.contains("private-source"), "{error}");
}

#[test]
fn unrelated_output_preserves_pcm_and_incremental_progress() {
    let f = Folder::new();
    let e = imported(&f.join("missing-source.wav"), 48_000);
    let range = Range::new(0, 4096);
    let expected = soundcraft_mix::render_range(e.session(), range, 1024);
    assert!(expected.iter().flatten().any(|v| v.abs() > 0.01));
    let path = f.join("delivery.wav");
    let mut job = bounce(&e, &path).unwrap();
    let mut progress = vec![job.progress()];
    loop {
        if job.step(|| false).unwrap().is_some() {
            break;
        }
        progress.push(job.progress());
    }
    assert!(progress.len() >= 4 && progress.is_sorted());
    assert_eq!(job.progress(), 1.0);
    let (_, actual) = soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), None).unwrap();
    assert_eq!(actual.channels, expected);
}

#[test]
fn write_only_authority_does_not_require_read_access_to_decoded_demo_sources() {
    let f = Folder::new();
    let outside = Folder::new();
    let mut e = soundcraft_engine::demo::demo_engine();
    for (i, source) in e.session_mut().sources.iter_mut().enumerate() {
        source.path = outside.join(&format!("missing-{i}.wav")).to_str().unwrap().into();
    }
    let roots = AutomationRoots::new(None, Some(&f.0)).unwrap().unwrap();
    let path = f.join("demo.wav");
    confine(Some(&roots), || bounce(&e, &path).unwrap().run()).unwrap();
    let (_, decoded) = soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), None).unwrap();
    assert!(decoded.channels.iter().flatten().any(|v| v.abs() > 0.01));
}

#[test]
fn destination_write_rejection_precedes_unresolvable_source_locator() {
    let allowed = Folder::new();
    let outside = Folder::new();
    let mut e = Engine::default();
    video(&mut e, "private-missing-directory/../private-video.mp4");
    let roots = AutomationRoots::new(None, Some(&allowed.0)).unwrap().unwrap();
    let error = confine(Some(&roots), || refused(&e, &outside.join("output.wav")));
    assert!(error.contains("outside the write root"), "{error}");
    assert!(!error.contains("private-video") && !error.contains("private-missing-directory"));
}

#[test]
fn empty_source_locators_do_not_reserve_unrelated_outputs() {
    let f = Folder::new();
    let mut e = imported(&f.join("unused.wav"), 48_000);
    e.session_mut().sources[0].path.clear();
    video(&mut e, "");
    bounce(&e, &f.join("delivery.wav")).unwrap().run().unwrap();
}

#[cfg(unix)]
#[test]
fn missing_leaf_under_symlink_alias_resolves_to_reserved_name() {
    let f = Folder::new();
    let real = f.join("real");
    let alias = f.join("alias");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let e = imported(&real.join("take.wav"), 48_000);
    refused(&e, &alias.join("take.wav"));
}

#[cfg(unix)]
#[test]
fn dangling_source_locator_fails_closed_with_generic_error() {
    let f = Folder::new();
    let dangling = f.join("private-dangling.wav");
    std::os::unix::fs::symlink(f.join("private-missing-target.wav"), &dangling).unwrap();
    let e = imported(&dangling, 48_000);
    let error = refused(&e, &f.join("delivery.wav"));
    assert!(error.contains("bounce media locator name cannot be validated"), "{error}");
    assert!(!error.contains("private-dangling") && !error.contains("private-missing-target"), "{error}");
}

#[cfg(unix)]
#[test]
fn non_utf8_output_name_is_preserved_losslessly() {
    use std::os::unix::ffi::OsStringExt;
    let f = Folder::new();
    let e = imported(&f.join("source.wav"), 48_000);
    let name = std::ffi::OsString::from_vec(b"delivery-\xff.wav".to_vec());
    let path = f.0.join(name);
    bounce(&e, &path).unwrap().run().unwrap();
    let (_, decoded) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
    assert_eq!(decoded.frames(), 4096);
    assert!(std::fs::read_dir(&f.0).unwrap().any(|entry| entry.unwrap().path() == path));
}

#[test]
fn case_variant_of_a_current_media_name_is_conservatively_reserved() {
    let f = Folder::new();
    let e = imported(&f.join("Take.wav"), 48_000);
    let error = refused(&e, &f.join("take.wav"));
    assert!(error.contains("current media locator"), "{error}");
}

#[test]
fn relative_session_folder_resolves_against_captured_current_directory() {
    let cwd = std::env::current_dir().unwrap();
    let f = Folder::new();
    let name = format!("{}-relative-folder.wav", f.0.file_name().unwrap().to_str().unwrap());
    let mut e = imported(&f.join("unused.wav"), 48_000);
    e.session_mut().folder = Some(PathBuf::from("."));
    e.session_mut().sources[0].path = name.clone();
    let error = refused(&e, &cwd.join(name));
    assert!(error.contains("current media locator"), "{error}");
}

#[cfg(unix)]
#[test]
fn non_utf8_session_folder_preserves_bytes_while_reserving_ascii_case_alias() {
    use std::os::unix::ffi::OsStringExt;
    let f = Folder::new();
    let source_folder = f.0.join(std::ffi::OsString::from_vec(b"session-\xff-A".to_vec()));
    let destination_folder = f.0.join(std::ffi::OsString::from_vec(b"session-\xff-a".to_vec()));
    let mut e = imported(&f.join("unused.wav"), 48_000);
    e.session_mut().folder = Some(source_folder);
    e.session_mut().sources[0].path = "take.wav".into();
    let destination = destination_folder.join("take.wav");
    let error = bounce(&e, &destination).err().expect("native case alias must be reserved").to_string();
    assert!(error.contains("current media locator"), "{error}");
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0);
}
