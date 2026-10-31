//! Native Bounce publication owns its staging file, never an existing destination.
#![cfg(not(target_arch = "wasm32"))]

use soundcraft_engine::file_access::{AutomationRoots, confine};
use soundcraft_engine::io::Bounce;
use soundcraft_time::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path =
            std::env::temp_dir().join(format!("soundcraft-bounce-publication-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> PathBuf {
        self.0.join("Mix.wav")
    }

    fn entries(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().path()).collect()
    }

    fn bounce(&self) -> Bounce {
        start(&self.path()).unwrap()
    }

    fn stage(&self) -> PathBuf {
        let entries: Vec<_> = self.entries().into_iter().filter(|p| p != &self.path() && p.file_name().unwrap() != "Mix.wav.part").collect();
        assert_eq!(entries.len(), 1, "one owned sibling stage: {entries:?}");
        entries[0].clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn start(path: &Path) -> soundcraft_engine::Result<Bounce> {
    let e = soundcraft_engine::demo::demo_engine();
    Bounce::new(e.session_arc(), path, Range::new(0, 4096), &soundcraft_audio_io::EncodeOptions::default(), None, false, false)
}

#[test]
fn existing_file_is_preserved_at_construction() {
    let f = Fixture::new();
    std::fs::write(f.path(), b"existing destination").unwrap();
    assert!(start(&f.path()).is_err());
    assert_eq!(std::fs::read(f.path()).unwrap(), b"existing destination");
    assert_eq!(f.entries().len(), 1);
}

#[test]
fn existing_directory_is_preserved_at_construction() {
    let f = Fixture::new();
    std::fs::create_dir(f.path()).unwrap();
    assert!(start(&f.path()).is_err());
    assert!(f.path().is_dir());
    assert_eq!(f.entries().len(), 1);
}

#[test]
fn legacy_part_is_neither_opened_nor_removed() {
    let f = Fixture::new();
    let legacy = f.0.join("Mix.wav.part");
    std::fs::write(&legacy, b"unrelated legacy work").unwrap();
    f.bounce().run().unwrap();
    assert_eq!(std::fs::read(legacy).unwrap(), b"unrelated legacy work");
    assert_eq!(f.entries().len(), 2);
}

#[test]
fn cancelling_one_of_two_jobs_preserves_the_other_stage() {
    let f = Fixture::new();
    let first = f.bounce();
    let first_stage = f.stage();
    let second = f.bounce();
    assert_eq!(f.entries().len(), 2);
    drop(first);
    assert!(!first_stage.exists());
    assert_eq!(f.entries().len(), 1);
    second.run().unwrap();
    assert_eq!(f.entries(), vec![f.path()]);
}

#[test]
fn first_published_job_wins_without_second_overwrite() {
    let f = Fixture::new();
    let first = f.bounce();
    let second = f.bounce();
    first.run().unwrap();
    let bytes = std::fs::read(f.path()).unwrap();
    assert!(second.run().is_err());
    assert_eq!(std::fs::read(f.path()).unwrap(), bytes);
    assert_eq!(f.entries(), vec![f.path()]);
}

#[test]
fn concurrent_final_creator_wins_without_overwrite() {
    let f = Fixture::new();
    let bounce = f.bounce();
    std::fs::write(f.path(), b"winner").unwrap();
    assert!(bounce.run().is_err());
    assert_eq!(std::fs::read(f.path()).unwrap(), b"winner");
    assert_eq!(f.entries(), vec![f.path()]);
}

#[test]
fn replacing_stage_with_a_foreign_file_preserves_it_on_failure_and_drop() {
    let f = Fixture::new();
    let bounce = f.bounce();
    let stage = f.stage();
    // Move rather than unlink so the original inode cannot immediately be reused.
    let held = f.0.join("held-stage");
    std::fs::rename(&stage, &held).unwrap();
    std::fs::write(&stage, b"foreign stage").unwrap();
    assert!(bounce.run().is_err());
    assert_eq!(std::fs::read(stage).unwrap(), b"foreign stage");
    assert!(held.is_file(), "the displaced owned inode is retained");
    assert!(!f.path().exists());
}

#[test]
fn missing_stage_fails_without_publishing() {
    let f = Fixture::new();
    let bounce = f.bounce();
    std::fs::remove_file(f.stage()).unwrap();
    assert!(bounce.run().is_err());
    assert!(f.entries().is_empty());
}

#[test]
fn write_only_authority_can_render_and_publish_in_memory_audio() {
    let f = Fixture::new();
    let roots = AutomationRoots::new(&[], &[&f.0]).unwrap().unwrap();
    confine(Some(&roots), || f.bounce().run()).unwrap();
    assert!(f.path().is_file());
    assert_eq!(f.entries(), vec![f.path()]);
}

#[test]
fn narrower_current_authority_prevents_later_publication() {
    let f = Fixture::new();
    let other = Fixture::new();
    let bounce = f.bounce();
    let roots = AutomationRoots::new(&[], &[&other.0]).unwrap().unwrap();
    assert!(confine(Some(&roots), || bounce.run()).is_err());
    assert!(!f.path().exists());
    // Cleanup must not use the newly denied path as authority to remove arbitrary files.
}

#[test]
fn cancellation_under_narrower_authority_does_not_flush_buffered_bytes() {
    let f = Fixture::new();
    let other = Fixture::new();
    let mut bounce = f.bounce();
    assert_eq!(bounce.step(|| false).unwrap(), None);
    let stage = f.stage();
    let before = std::fs::read(&stage).unwrap();
    assert!(before.len() < 1024 * 2 * 3, "a rendered block remains buffered");
    let roots = AutomationRoots::new(&[], &[&other.0]).unwrap().unwrap();
    confine(Some(&roots), || drop(bounce));
    assert_eq!(std::fs::read(&stage).unwrap(), before);
    assert!(!f.path().exists());
}

#[test]
fn construction_authority_survives_leaving_its_scope() {
    let f = Fixture::new();
    let roots = AutomationRoots::new(&[], &[&f.0]).unwrap().unwrap();
    let bounce = confine(Some(&roots), || f.bounce());
    bounce.run().unwrap();
    assert_eq!(f.entries(), vec![f.path()]);
}

#[cfg(unix)]
#[test]
fn dangling_destination_link_is_preserved() {
    let f = Fixture::new();
    let target = f.0.join("absent");
    std::os::unix::fs::symlink(&target, f.path()).unwrap();
    assert!(start(&f.path()).is_err());
    assert_eq!(std::fs::read_link(f.path()).unwrap(), target);
    assert_eq!(f.entries().len(), 1);
}

#[cfg(unix)]
#[test]
fn replacing_stage_with_a_link_never_modifies_its_target() {
    let f = Fixture::new();
    let bounce = f.bounce();
    let stage = f.stage();
    let target = f.0.join("foreign");
    std::fs::write(&target, b"foreign target").unwrap();
    std::fs::remove_file(&stage).unwrap();
    std::os::unix::fs::symlink(&target, &stage).unwrap();
    assert!(bounce.run().is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"foreign target");
    assert_eq!(std::fs::read_link(stage).unwrap(), target);
    assert!(!f.path().exists());
}

#[cfg(unix)]
#[test]
fn redirecting_parent_does_not_publish_or_remove_foreign_bytes() {
    let f = Fixture::new();
    let parent = f.0.join("parent");
    let moved = f.0.join("moved");
    let foreign = f.0.join("foreign");
    std::fs::create_dir(&parent).unwrap();
    std::fs::create_dir(&foreign).unwrap();
    let path = parent.join("Mix.wav");
    let bounce = start(&path).unwrap();
    let stage_name = std::fs::read_dir(&parent).unwrap().next().unwrap().unwrap().file_name();
    std::fs::rename(&parent, &moved).unwrap();
    std::fs::write(foreign.join(&stage_name), b"foreign stage").unwrap();
    std::os::unix::fs::symlink(&foreign, &parent).unwrap();
    assert!(bounce.run().is_err());
    assert_eq!(std::fs::read(foreign.join(stage_name)).unwrap(), b"foreign stage");
    assert!(!foreign.join("Mix.wav").exists());
    assert!(!moved.join("Mix.wav").exists());
}

#[cfg(unix)]
#[test]
fn captured_authority_rejects_parent_escape_after_scope_ends() {
    let f = Fixture::new();
    let outside = Fixture::new();
    let parent = f.0.join("parent");
    std::fs::create_dir(&parent).unwrap();
    let roots = AutomationRoots::new(&[], &[&f.0]).unwrap().unwrap();
    let path = parent.join("Mix.wav");
    let bounce = confine(Some(&roots), || start(&path)).unwrap();
    let stage_name = std::fs::read_dir(&parent).unwrap().next().unwrap().unwrap().file_name();
    std::fs::rename(&parent, f.0.join("moved")).unwrap();
    std::fs::write(outside.0.join(&stage_name), b"outside stage").unwrap();
    std::os::unix::fs::symlink(&outside.0, &parent).unwrap();
    assert!(bounce.run().is_err());
    assert_eq!(std::fs::read(outside.0.join(stage_name)).unwrap(), b"outside stage");
    assert!(!outside.path().exists());
}

#[test]
fn unsupported_encoder_initialization_cleans_its_stage() {
    let f = Fixture::new();
    let e = soundcraft_engine::demo::demo_engine();
    let opts = soundcraft_audio_io::EncodeOptions {
        format: soundcraft_audio_io::FileFormat::Flac,
        bit_depth: soundcraft_audio_io::BitDepth::Float32,
        dither: false,
        bwf: None,
    };
    assert!(Bounce::new(e.session_arc(), &f.path(), Range::new(0, 4096), &opts, None, false, false).is_err());
    assert!(f.entries().is_empty());
}

#[test]
fn late_destination_directory_is_preserved_and_stage_cleaned() {
    let f = Fixture::new();
    let bounce = f.bounce();
    std::fs::create_dir(f.path()).unwrap();
    assert!(bounce.run().is_err());
    assert!(f.path().is_dir());
    assert_eq!(f.entries(), vec![f.path()]);
}

#[cfg(unix)]
#[test]
fn existing_destination_link_and_legacy_part_link_preserve_targets() {
    let f = Fixture::new();
    let target = f.0.join("foreign");
    let legacy = f.0.join("Mix.wav.part");
    std::fs::write(&target, b"foreign bytes").unwrap();
    std::os::unix::fs::symlink(&target, f.path()).unwrap();
    assert!(start(&f.path()).is_err());
    assert_eq!(std::fs::read_link(f.path()).unwrap(), target);
    std::fs::remove_file(f.path()).unwrap();
    std::os::unix::fs::symlink(&target, &legacy).unwrap();
    f.bounce().run().unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"foreign bytes");
    assert_eq!(std::fs::read_link(legacy).unwrap(), target);
    assert_eq!(f.entries().len(), 3);
}

#[cfg(unix)]
#[test]
fn late_dangling_destination_is_preserved_and_owned_stage_cleaned() {
    let f = Fixture::new();
    let roots = AutomationRoots::new(&[], &[&f.0]).unwrap().unwrap();
    let bounce = confine(Some(&roots), || f.bounce());
    let target = f.0.join("absent");
    std::os::unix::fs::symlink(&target, f.path()).unwrap();
    assert!(bounce.run().is_err());
    assert_eq!(std::fs::read_link(f.path()).unwrap(), target);
    assert_eq!(f.entries(), vec![f.path()]);
}
