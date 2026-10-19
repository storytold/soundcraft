//! Synthetic audio-portable copies: missing media never becomes a successful archive claim.
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, io, session_copy};
use soundcraft_model::{ChannelFormat, Clip, Insert, Playlist, SourceAudio, SourceId, TrackKind, VideoSource};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("soundcraft-copy-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn audio() -> Engine {
    let mut e = Engine::default();
    let s = e.session_mut();
    for width in [1, 2, 4] {
        let track = s.add_track(TrackKind::Audio, ChannelFormat::for_channels(width), Some(&format!("Width {width}")));
        let channels = (0..width).map(|ch| (0..16).map(|frame| (ch + 1) as f32 * (frame + 1) as f32 / 128.0).collect()).collect();
        let src = io::add_source(s, "Tone", AudioBuffer { sample_rate: 48_000, channels }, Some("/missing-original/shared.wav"), FileFormat::Wav);
        let clip = s.new_clip_id();
        s.track_mut(track).unwrap().playlists[0].clips.push(Clip::audio(clip, "tone", src, 0, 0, 16));
    }
    io::add_source(s, "Unused", AudioBuffer::new(48_000, 1, 8), None, FileFormat::Wav);
    let alternate = io::add_source(s, "Alternate only", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.15; 16]] }, None, FileFormat::Wav);
    let clip = s.new_clip_id();
    let mut pl = Playlist::new("alternate");
    pl.clips.push(Clip::audio(clip, "alternate only", alternate, 1, 4, 8));
    s.tracks[0].playlists.push(pl);
    let instrument = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Sampler"));
    let mut sampler = Insert::new("sampler");
    sampler.sample = Some(s.sources[2].id);
    s.track_mut(instrument).unwrap().instrument = Some(sampler);
    e
}

#[test]
fn moved_copy_reopens_every_declared_source_and_playlist_without_originals() {
    let dir = Fixture::new();
    let mut e = audio();
    io::save_session(&mut e, &dir.path("original/Song.scraft")).unwrap();
    let before = e.session_arc();
    let result = e.execute("session.save_copy", &json!({"path": dir.path("copy/Copy.scraft")})).unwrap();
    assert_eq!(result["audio_complete"], true);
    assert_eq!(result["full_session_portability"], "not_assessed");
    assert!(result.get("session_complete").is_none());
    assert_eq!(result["audio_sources_packaged"], 5);
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    std::fs::rename(dir.0.join("copy"), dir.0.join("moved")).unwrap();
    std::fs::remove_dir_all(dir.0.join("original")).unwrap();
    let mut reopened = Engine::default();
    assert!(io::open_session(&mut reopened, &dir.path("moved/Copy.scraft")).unwrap().is_empty());
    let paths: std::collections::BTreeSet<_> = reopened.session().sources.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths.len(), 5);
    for src in &before.sources {
        let original = &before.pool.get(src.id).unwrap().buffer;
        let copied = &reopened.session().pool.get(src.id).unwrap().buffer;
        assert_eq!(original, copied, "all declared audio, including unused samples");
    }
    assert_eq!(before.tracks, reopened.session().tracks);
    for track in &before.tracks {
        assert_eq!(
            soundcraft_mix::render_clips(&before, track.id, soundcraft_time::Range::new(0, 16)),
            soundcraft_mix::render_clips(reopened.session(), track.id, soundcraft_time::Range::new(0, 16))
        );
    }
    let mut original_alternate = (*before).clone();
    original_alternate.tracks[0].active_playlist = 1;
    let mut copied_alternate = reopened.session().clone();
    copied_alternate.tracks[0].active_playlist = 1;
    let track = original_alternate.tracks[0].id;
    assert_eq!(
        soundcraft_mix::render_clips(&original_alternate, track, soundcraft_time::Range::new(0, 16)),
        soundcraft_mix::render_clips(&copied_alternate, track, soundcraft_time::Range::new(0, 16))
    );
}

#[test]
fn missing_relative_and_absolute_sources_reject_before_any_destination_write() {
    for relative in [false, true] {
        let dir = Fixture::new();
        let mut e = audio();
        let s = e.session_mut();
        let id = s.sources[1].id;
        s.pool.remove(id);
        s.folder = Some(dir.0.join("original"));
        s.sources[1].path = if relative { "Audio Files/Missing.wav".into() } else { dir.path("original/Missing.wav") };
        let before = e.session_arc();
        let preview = e.execute("session.save_copy_preview", &json!({"path": dir.path("copy/Copy.scraft")})).unwrap();
        assert_eq!(preview["ready"], false);
        assert_eq!(preview["missing_audio"], 1);
        assert!(
            e.execute("session.save_copy", &json!({"path": dir.path("copy/Copy.scraft")}))
                .unwrap_err()
                .to_string()
                .contains(&format!("source {}", id.0))
        );
        assert!(!dir.0.exists(), "an available first source must not be written before later missing media is detected");
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        // Normal saves and recovery retain missing-media documents.
        io::save_session(&mut e, &dir.path("normal/Song.scraft")).unwrap();
        io::save_copy(&e, &dir.path("recovery/Recovery.scraft")).unwrap();
        let mut r = Engine::default();
        assert_eq!(io::open_session(&mut r, &dir.path("recovery/Recovery.scraft")).unwrap().len(), 1);
        assert_eq!(r.session().sources.len(), 5);
    }
}

#[test]
fn replacement_gate_and_missing_media_preserve_existing_destination() {
    let dir = Fixture::new();
    let mut first = audio();
    io::save_session(&mut first, &dir.path("copy/Copy.scraft")).unwrap();
    let old = std::fs::read(dir.0.join("copy/Copy.scraft")).unwrap();
    let mut second = audio();
    assert!(second.execute("session.save_copy", &json!({"path": dir.path("copy/Copy.scraft")})).unwrap_err().to_string().contains("replace: true"));
    let missing = second.session().sources[1].id;
    second.session_mut().pool.remove(missing);
    assert!(second.execute("session.save_copy", &json!({"path": dir.path("copy/Copy.scraft"), "replace": true})).is_err());
    assert_eq!(std::fs::read(dir.0.join("copy/Copy.scraft")).unwrap(), old);
    assert!(!dir.0.join("copy/Backups").exists());
    assert_eq!(std::fs::read_dir(dir.0.join("copy/Audio Files")).unwrap().count(), 5);
}

#[test]
fn same_folder_copy_validates_bytes_and_keeps_prior_sessions_audio() {
    let dir = Fixture::new();
    let mut e = audio();
    io::save_session(&mut e, &dir.path("Song.scraft")).unwrap();
    let src = e.session().sources[0].clone();
    let old_path = dir.0.join(&src.path);
    let old = std::fs::read(&old_path).unwrap();
    e.session_mut().pool.insert(src.id, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![-0.25; 16]] })));
    let result = e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).unwrap();
    assert_eq!(result["audio_files_written"], 1);
    assert_eq!(std::fs::read(&old_path).unwrap(), old);
    let mut r = Engine::default();
    assert!(io::open_session(&mut r, &dir.path("Copy.scraft")).unwrap().is_empty());
    assert_eq!(r.session().pool.get(src.id).unwrap().buffer.channels[0], vec![-0.25; 16]);
    let again = e.execute("session.save_copy", &json!({"path": dir.path("Copy 2.scraft")})).unwrap();
    assert_eq!(again["audio_files_written"], 0);
}

#[test]
fn copy_sanitizes_nonfinite_output_without_changing_original_pcm_bits() {
    let dir = Fixture::new();
    let mut e = audio();
    let src = e.session().sources[0].id;
    let values = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25].repeat(4);
    let original_bits: Vec<_> = values.iter().map(|sample| sample.to_bits()).collect();
    e.session_mut().pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![values] })));
    e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).unwrap();
    let mut r = Engine::default();
    assert!(io::open_session(&mut r, &dir.path("Copy.scraft")).unwrap().is_empty());
    assert!(r.session().pool.get(src).unwrap().buffer.channels[0].iter().all(|sample| sample.is_finite()));
    assert_eq!(e.session().pool.get(src).unwrap().buffer.channels[0].iter().map(|sample| sample.to_bits()).collect::<Vec<_>>(), original_bits);
}

#[test]
fn copied_audio_is_repaired_when_same_sized_existing_media_was_changed() {
    let dir = Fixture::new();
    let mut e = audio();
    e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).unwrap();
    let text = std::fs::read_to_string(dir.0.join("Copy.scraft")).unwrap();
    let copy = soundcraft_model::Session::from_json(&text).unwrap();
    let path = dir.0.join(&copy.sources[0].path);
    let mut wrong = std::fs::read(&path).unwrap();
    let last = wrong.last_mut().unwrap();
    *last ^= 0x01;
    std::fs::write(&path, &wrong).unwrap();
    let result = e.execute("session.save_copy", &json!({"path": dir.path("Copy 2.scraft")})).unwrap();
    assert_eq!(result["audio_files_written"], 1);
    assert_eq!(std::fs::read(&path).unwrap(), wrong);
    let mut r = Engine::default();
    assert!(io::open_session(&mut r, &dir.path("Copy 2.scraft")).unwrap().is_empty());
    assert_eq!(r.session().pool.get(copy.sources[0].id).unwrap().buffer, e.session().pool.get(copy.sources[0].id).unwrap().buffer);
}

#[test]
fn malformed_audio_and_references_block_without_output() {
    for case in 0..10 {
        let dir = Fixture::new();
        let mut e = audio();
        let s = e.session_mut();
        let id = s.sources[0].id;
        match case {
            0 => s.sources.push(s.sources[0].clone()),
            1 => s.tracks[0].playlists[1].clips[0].content = soundcraft_model::ClipContent::Audio { source: SourceId(999), offset: 0 },
            2 => s.tracks[3].instrument.as_mut().unwrap().sample = Some(SourceId(999)),
            3 => {
                let mut insert = Insert::new("sampler");
                insert.sample = Some(SourceId(999));
                s.tracks[0].mixer.inserts[0] = Some(insert);
            }
            4 => {
                s.sources.clear();
            }
            5 => s.sources[0].frames += 1,
            6 => s.sources[0].sample_rate = 44_100,
            7 => s.pool.insert(id, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 16], vec![0.0; 15]] }))),
            8 => s.pool.insert(id, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 0, channels: vec![vec![0.0; 16]] }))),
            _ => s.pool.insert(id, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![] }))),
        }
        assert!(!session_copy::preview(&e, None).ready, "case {case}");
        assert!(e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).is_err(), "case {case}");
        assert!(!dir.0.exists());
        assert!(session_copy::bundle(&e, "Copy.scraftz").is_err());
    }
}

#[test]
fn copy_and_preview_preserve_arc_revision_dirty_path_undo_redo_and_journal() {
    let dir = Fixture::new();
    let mut e = audio();
    e.execute("mix.volume", &json!({"track": "Width 1", "db": -6})).unwrap();
    e.execute("track.new", &json!({"name": "Undo me"})).unwrap();
    assert!(e.undo());
    let (before, revision, dirty, path, history, redo, journal) =
        (e.session_arc(), e.revision, e.is_dirty(), e.path.clone(), e.undo_history(), e.redo_label().map(str::to_string), e.journal.clone());
    e.execute("session.save_copy_preview", &json!({"path": dir.path("Copy.scraft")})).unwrap();
    e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).unwrap();
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert_eq!(
        (e.revision, e.is_dirty(), e.path.clone(), e.undo_history(), e.redo_label().map(str::to_string), e.journal.clone()),
        (revision, dirty, path, history, redo, journal)
    );
    assert!(e.redo());
    assert!(e.session().track_by_name("Undo me").is_some());
    assert!(e.undo());
    assert_eq!(e.session(), &*before);
}

#[test]
fn strict_bundle_round_trips_audio_and_reports_external_video_honestly() {
    let dir = Fixture::new();
    let mut e = audio();
    e.session_mut().videos.push(VideoSource {
        id: SourceId(999),
        name: "Picture".into(),
        path: "/external/picture.mov".into(),
        width: 16,
        height: 9,
        frame_rate: 24.0,
        duration: 1.0,
        codec: "mjpeg".into(),
    });
    let before = e.session_arc();
    let (bytes, result) = session_copy::bundle(&e, &dir.path("missing-dir/Copy.scraftz")).unwrap();
    assert!(result.audio_complete);
    assert_eq!(result.full_session_portability, "not_assessed");
    assert_eq!(result.media.last().unwrap().status, "external_unverified");
    assert!(!dir.0.exists());
    let mut r = Engine::default();
    assert_eq!(io::open_session_bytes(&mut r, &result.output_path, &bytes).unwrap(), vec!["/external/picture.mov"]);
    for src in &before.sources {
        assert_eq!(before.pool.get(src.id).unwrap().buffer, r.session().pool.get(src.id).unwrap().buffer);
    }
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
}

#[test]
fn excessive_source_count_and_metadata_are_rejected_before_output() {
    let dir = Fixture::new();
    let mut e = audio();
    let src = e.session().sources[0].clone();
    e.session_mut().sources.resize(10_000, src);
    assert!(!session_copy::preview(&e, None).ready);
    assert!(e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).is_err());
    let mut e = Engine::default();
    e.session_mut().comments = "x".repeat((16 << 20) + 1);
    assert!(e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).is_err());
    assert!(!dir.0.exists());
}

#[test]
fn audio_free_session_can_be_copied_but_own_file_cannot_be_replaced() {
    let dir = Fixture::new();
    let mut e = Engine::default();
    assert_eq!(e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).unwrap()["audio_sources_packaged"], 0);
    io::open_session(&mut e, &dir.path("Copy.scraft")).unwrap();
    assert!(e.execute("session.save_copy", &json!({"path": dir.path("./Copy.scraft"), "replace": true})).is_err());
}

#[test]
fn commands_reject_wrong_types_unknown_options_and_stale_revisions() {
    let dir = Fixture::new();
    let mut e = audio();
    for params in [
        json!(null),
        json!([]),
        json!({"path": 5}),
        json!({"path": dir.path("Copy.scraft"), "replace": "yes"}),
        json!({"path": dir.path("Copy.scraft"), "sample_rate": 44100}),
        json!({"path": dir.path("Copy.scraft"), "expected_revision": 0}),
        json!({"path": dir.path("Copy.scraft"), "expected_revision": 1.5}),
    ] {
        assert!(e.execute("session.save_copy", &params).is_err(), "{params}");
    }
    for params in [json!(false), json!({"path": null}), json!({"replace": true}), json!({"unknown": 1}), json!({"expected_revision": "1"})] {
        assert!(e.execute("session.save_copy_preview", &params).is_err(), "{params}");
    }
    assert!(e.execute("session.save_copy_preview", &json!({"expected_revision": e.revision})).unwrap()["ready"].as_bool().unwrap());
    assert!(!dir.0.exists());
}

#[test]
fn parsed_out_of_range_session_and_matching_source_rates_cannot_be_exported() {
    let dir = Fixture::new();
    let mut value = serde_json::to_value(audio().session()).unwrap();
    value["sample_rate"] = json!(1);
    let session = soundcraft_model::Session::from_json(&value.to_string()).unwrap();
    let mut e = Engine::new(session);
    for src in e.session_mut().sources.iter_mut() {
        src.sample_rate = 1;
    }
    let ids: Vec<_> = e.session().sources.iter().map(|s| (s.id, s.channels, s.frames)).collect();
    for (id, channels, frames) in ids {
        e.session_mut().pool.insert(id, Arc::new(SourceAudio::new(AudioBuffer::new(1, usize::from(channels), frames as usize))));
    }
    assert!(!session_copy::preview(&e, None).ready);
    assert!(e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).is_err());
    assert!(!dir.0.exists());
}

#[cfg(unix)]
#[test]
fn destination_symlinks_and_aliases_fail_before_media_write() {
    use std::os::unix::fs::symlink;
    let dir = Fixture::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let mut e = audio();
    io::save_session(&mut e, &dir.path("original/Song.scraft")).unwrap();
    let original = std::fs::read(dir.0.join("original/Song.scraft")).unwrap();
    symlink(dir.0.join("original"), dir.0.join("alias")).unwrap();
    assert!(e.execute("session.save_copy", &json!({"path": dir.path("alias/Song.scraft"), "replace": true})).is_err());
    std::fs::create_dir_all(dir.0.join("copy")).unwrap();
    symlink(dir.0.join("original/Audio Files"), dir.0.join("copy/Audio Files")).unwrap();
    assert!(e.execute("session.save_copy", &json!({"path": dir.path("copy/Copy.scraft")})).is_err());
    assert!(!dir.0.join("copy/Copy.scraft").exists());
    assert_eq!(std::fs::read(dir.0.join("original/Song.scraft")).unwrap(), original);
}

#[test]
fn old_fixed_temporary_file_is_preserved_while_copy_uses_unique_staging() {
    let dir = Fixture::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let old = dir.0.join("Copy.scraft.tmp");
    let unrelated = b"unrelated user recording, never a Copy staging file";
    std::fs::write(&old, unrelated).unwrap();
    let mut e = audio();
    assert!(e.execute("session.save_copy", &json!({"path": dir.path("Copy.scraft")})).unwrap()["audio_complete"].as_bool().unwrap());
    assert_eq!(std::fs::read(&old).unwrap(), unrelated);
    let mut reopened = Engine::default();
    assert!(io::open_session(&mut reopened, &dir.path("Copy.scraft")).unwrap().is_empty());
    assert_eq!(reopened.session().sources.len(), 5);
    assert!(std::fs::read_dir(&dir.0).unwrap().flatten().all(|entry| !entry.file_name().to_string_lossy().starts_with(".scraft-copy-")));
}

#[cfg(unix)]
#[test]
fn old_fixed_temporary_symlink_and_hardlink_preserve_unrelated_and_original_bytes() {
    use std::os::unix::fs::symlink;
    let dir = Fixture::new();
    let mut e = audio();
    io::save_session(&mut e, &dir.path("original/Song.scraft")).unwrap();
    let original = std::fs::read(dir.0.join("original/Song.scraft")).unwrap();
    let unrelated = b"unrelated user audio";
    std::fs::write(dir.0.join("unrelated.wav"), unrelated).unwrap();
    for (name, target, hardlink) in [
        ("unrelated-symlink", dir.0.join("unrelated.wav"), false),
        ("unrelated-hardlink", dir.0.join("unrelated.wav"), true),
        ("original-symlink", dir.0.join("original/Song.scraft"), false),
        ("original-hardlink", dir.0.join("original/Song.scraft"), true),
    ] {
        let folder = dir.0.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let old = folder.join("Copy.scraft.tmp");
        if hardlink {
            std::fs::hard_link(&target, &old).unwrap();
        } else {
            symlink(&target, &old).unwrap();
        }
        let before = std::fs::read(&old).unwrap();
        let result = e.execute("session.save_copy", &json!({"path": dir.path(&format!("{name}/Copy.scraft"))})).unwrap();
        assert_eq!(result["audio_complete"], true);
        assert_eq!(std::fs::read(&old).unwrap(), before);
        assert_eq!(std::fs::read(&target).unwrap(), before);
        if !hardlink {
            assert_eq!(std::fs::read_link(&old).unwrap(), target);
        }
        assert!(std::fs::read_dir(&folder).unwrap().flatten().all(|entry| !entry.file_name().to_string_lossy().starts_with(".scraft-copy-")));
    }
    assert_eq!(std::fs::read(dir.0.join("unrelated.wav")).unwrap(), unrelated);
    assert_eq!(std::fs::read(dir.0.join("original/Song.scraft")).unwrap(), original);
}

#[test]
fn automation_write_root_blocks_copy_preview_publication_and_symlink_escape() {
    use soundcraft_engine::file_access::{AutomationRoots, confine};
    let dir = Fixture::new();
    let inside = dir.0.join("inside");
    let outside = dir.0.join("outside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let existing = outside.join("Sentinel.scraft");
    std::fs::write(&existing, b"external session").unwrap();
    let roots = AutomationRoots::new(None, Some(&inside)).unwrap().unwrap();
    let mut e = audio();
    let document = e.session_arc();
    let revision = e.revision;
    let journal = e.journal.clone();
    let undo = e.undo_history();
    confine(Some(&roots), || {
        for path in [outside.join("New.scraft"), existing.clone()] {
            let params = json!({"path": path});
            let preview = e.execute("session.save_copy_preview", &params).unwrap();
            assert_eq!(preview["ready"], false);
            assert!(preview["issues"].to_string().contains("write root"));
            let error = e.execute("session.save_copy", &params).unwrap_err().to_string();
            assert!(error.contains("write root"), "reject before probing an outside replacement: {error}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, inside.join("escape")).unwrap();
            let params = json!({"path":inside.join("escape/Escaped.scraft")});
            assert_eq!(e.execute("session.save_copy_preview", &params).unwrap()["ready"], false);
            assert!(e.execute("session.save_copy", &params).unwrap_err().to_string().contains("write root"));
        }
        let permitted = inside.join("fresh/Copy.scraft");
        let result = e.execute("session.save_copy", &json!({"path":permitted})).unwrap();
        assert_eq!(result["audio_complete"], true);
        assert_eq!(result["audio_sources_packaged"], 5);
        assert!(permitted.is_file(), "fresh in-memory audio needs no read authority");
    });
    assert_eq!(std::fs::read(&existing).unwrap(), b"external session");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    assert!(Arc::ptr_eq(&document, &e.session_arc()));
    assert_eq!(e.revision, revision);
    assert_eq!(e.journal, journal);
    assert_eq!(e.undo_history(), undo);
}

#[test]
fn copy_without_read_authority_encodes_fresh_media_and_reuses_only_after_read_grant() {
    use soundcraft_engine::file_access::{AutomationRoots, confine};
    let dir = Fixture::new();
    let destination = dir.0.join("copies");
    std::fs::create_dir_all(&destination).unwrap();
    let writes_only = AutomationRoots::new(None, Some(&destination)).unwrap().unwrap();
    let mut e = audio();
    let first = destination.join("First.scraft");
    confine(Some(&writes_only), || {
        let result = e.execute("session.save_copy", &json!({"path":first})).unwrap();
        assert_eq!(result["audio_files_written"], 5);
    });
    let original_session = std::fs::read(&first).unwrap();
    let audio_dir = destination.join("Audio Files");
    let audio_before: std::collections::BTreeMap<_, _> = std::fs::read_dir(&audio_dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let second = destination.join("Second.scraft");
    confine(Some(&writes_only), || {
        let replacement = json!({"path":first, "replace":true});
        let preview = e.execute("session.save_copy_preview", &json!({"path":first})).unwrap();
        assert_eq!(preview["ready"], false);
        assert!(preview["issues"].to_string().contains("read authority is absent"));
        assert!(e.execute("session.save_copy", &replacement).unwrap_err().to_string().contains("read authority is absent"));
        let params = json!({"path":second});
        assert_eq!(e.execute("session.save_copy_preview", &params).unwrap()["ready"], true);
        let result = e.execute("session.save_copy", &params).unwrap();
        assert_eq!(result["audio_files_written"], 5);
        assert!(result["media"].as_array().unwrap().iter().all(|row| row["status"] == "written"));
    });
    assert!(second.is_file());
    assert_eq!(std::fs::read(&first).unwrap(), original_session);
    for (path, bytes) in &audio_before {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 3);
    assert_eq!(std::fs::read_dir(&audio_dir).unwrap().count(), 10);
    let read_write = AutomationRoots::new(Some(&destination), Some(&destination)).unwrap().unwrap();
    let third = destination.join("Third.scraft");
    confine(Some(&read_write), || {
        let params = json!({"path":third});
        assert_eq!(e.execute("session.save_copy_preview", &params).unwrap()["ready"], true);
        let result = e.execute("session.save_copy", &params).unwrap();
        assert_eq!(result["audio_files_written"], 0);
        assert!(result["media"].as_array().unwrap().iter().all(|row| row["status"] == "reused"));
        let mut reopened = Engine::default();
        assert!(io::open_session(&mut reopened, &third.to_string_lossy()).unwrap().is_empty());
        assert_eq!(reopened.session().sources.len(), 5);
    });
    assert_eq!(std::fs::read_dir(&audio_dir).unwrap().count(), 10);
    assert_eq!(std::fs::read(&first).unwrap(), original_session);
}

#[test]
fn loaded_original_outside_authorized_roots_copies_to_a_fresh_destination() {
    use soundcraft_engine::file_access::{AutomationRoots, confine};
    let dir = Fixture::new();
    let mut e = audio();
    let original = dir.0.join("original/Song.scraft");
    io::save_session(&mut e, &original.to_string_lossy()).unwrap();
    let original_bytes = std::fs::read(&original).unwrap();
    let document = e.session_arc();
    let revision = e.revision;
    let journal = e.journal.clone();
    let destination = dir.0.join("copies");
    std::fs::create_dir_all(&destination).unwrap();
    let roots = AutomationRoots::new(None, Some(&destination)).unwrap().unwrap();
    let copy = destination.join("Copy.scraft");
    confine(Some(&roots), || {
        let params = json!({"path":copy});
        assert_eq!(e.execute("session.save_copy_preview", &params).unwrap()["ready"], true);
        assert_eq!(e.execute("session.save_copy", &params).unwrap()["audio_complete"], true);
        let replacement = json!({"path":copy, "replace":true});
        let preview = e.execute("session.save_copy_preview", &json!({"path":copy})).unwrap();
        assert_eq!(preview["ready"], false);
        assert!(preview["issues"].to_string().contains("open-session file identity cannot be verified"));
        assert!(e.execute("session.save_copy", &replacement).is_err());
    });
    assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
    assert!(Arc::ptr_eq(&document, &e.session_arc()));
    assert_eq!(e.revision, revision);
    assert_eq!(e.journal, journal);
    let mut reopened = Engine::default();
    assert!(io::open_session(&mut reopened, &copy.to_string_lossy()).unwrap().is_empty());
    assert_eq!(reopened.session().sources.len(), 5);
    #[cfg(unix)]
    {
        let blocked_dir = destination.join("blocked");
        let media = blocked_dir.join("Audio Files");
        std::fs::create_dir_all(&media).unwrap();
        std::os::unix::fs::symlink(dir.0.join("original/Audio Files/Tone.wav"), media.join("Tone.wav")).unwrap();
        let blocked = blocked_dir.join("Blocked.scraft");
        confine(Some(&roots), || {
            let params = json!({"path":blocked});
            // No directory enumeration without read authority; a collision is rejected when
            // the guarded exclusive writer encounters it, without reading its target.
            assert_eq!(e.execute("session.save_copy_preview", &params).unwrap()["ready"], true);
            assert!(e.execute("session.save_copy", &params).unwrap_err().to_string().contains("write root"));
        });
        assert!(!blocked.exists());
        assert_eq!(std::fs::read_dir(&media).unwrap().count(), 1);
        assert!(std::fs::symlink_metadata(media.join("Tone.wav")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
    }
}

#[test]
fn replacement_gate_precedes_unknown_options_without_accepting_them() {
    let dir = Fixture::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let existing = dir.0.join("Existing.scraft");
    let fresh = dir.0.join("Fresh.scraft");
    let sentinel = b"unchanged destination session";
    std::fs::write(&existing, sentinel).unwrap();
    let mut e = audio();
    let track = e.session().tracks[0].id;
    e.execute("mix.volume", &json!({"track":track,"db":-6})).unwrap();
    assert!(e.undo());
    let document = e.session_arc();
    let revision = e.revision;
    let dirty = e.is_dirty();
    let path = e.path.clone();
    let undo = e.undo_history();
    let redo = e.redo_label().map(str::to_string);
    let journal = e.journal.clone();
    for (params, replacement_error) in [
        (json!({"path":existing,"tracks":[track]}), true),
        (json!({"path":existing,"tracks":[track],"replace":false}), true),
        (json!({"path":existing,"tracks":[track],"replace":true}), false),
        (json!({"path":fresh,"tracks":[track]}), false),
        (json!({"path":fresh,"tracks":[track],"replace":true}), false),
    ] {
        let error = e.execute("session.save_copy", &params).unwrap_err().to_string();
        if replacement_error {
            assert!(error.contains("`replace: true`"), "{error}");
            assert!(!error.contains("unknown parameter"), "{error}");
        } else {
            assert!(error.contains("unknown parameter `tracks`"), "{error}");
        }
        assert_eq!(std::fs::read(&existing).unwrap(), sentinel);
        assert!(!fresh.exists());
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1, "no audio, session or staging output for rejected options");
        assert!(Arc::ptr_eq(&document, &e.session_arc()));
        assert_eq!(e.revision, revision);
        assert_eq!(e.is_dirty(), dirty);
        assert_eq!(e.path, path);
        assert_eq!(e.undo_history(), undo);
        assert_eq!(e.redo_label(), redo.as_deref());
        assert_eq!(e.journal, journal);
    }
    let error = e.execute("session.save_copy_preview", &json!({"path":existing,"tracks":[track]})).unwrap_err().to_string();
    assert!(error.contains("unknown parameter `tracks`"), "preview retains strict option validation: {error}");
    assert_eq!(std::fs::read(&existing).unwrap(), sentinel);
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    assert!(Arc::ptr_eq(&document, &e.session_arc()));
}
