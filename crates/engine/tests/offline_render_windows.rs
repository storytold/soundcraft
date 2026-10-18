//! Checked offline windows fail before destructive editing or output staging.
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::file_access::{AutomationRoots, confine};
use soundcraft_engine::io::{Bounce, add_source, commit_track};
use soundcraft_model::{BusId, ChannelFormat, Clip, Insert, Session, TrackId, TrackKind};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path =
                std::env::temp_dir().join(format!("soundcraft-checked-window-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("owned fixture: {err}"),
            }
        }
        panic!("no unused fixture directory");
    }
    fn output(&self) -> PathBuf {
        self.0.join("delivery.wav")
    }
    fn empty(&self) -> bool {
        std::fs::read_dir(&self.0).unwrap().next().is_none()
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn opts() -> EncodeOptions {
    EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None }
}

fn add_track(s: &mut Session, name: &str, format: ChannelFormat, end: i64) -> TrackId {
    let track = s.add_track(TrackKind::Audio, format, Some(name));
    let source = add_source(s, name, AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 64]] }, None, FileFormat::Wav);
    let clip = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, name, source, 0, 0, end));
    track
}

fn engine() -> (Engine, TrackId) {
    let mut s = Session::default();
    let track = add_track(&mut s, "Source", ChannelFormat::Mono, 64);
    (Engine::new(s), track)
}

fn start(e: &Engine, path: &Path, range: Range, bus: Option<BusId>, normalize: bool) -> soundcraft_engine::Result<Bounce> {
    Bounce::new(e.session_arc(), path, range, &opts(), bus, normalize, false)
}

fn unchanged_failure(e: &mut Engine, command: &str, tracks: &[TrackId]) {
    // Keep a pending Redo operation so a rejected command cannot erase history unnoticed.
    e.execute("track.new", &json!({"count": 1, "format": "mono"})).unwrap();
    assert!(e.undo());
    assert!(e.can_redo());
    let before = serde_json::to_value(e.session()).unwrap();
    let history = e.undo_history();
    let dirty = e.is_dirty();
    let pool = e.session().pool.len();
    let ids: Vec<_> = tracks.iter().map(|t| t.0).collect();
    assert!(e.execute(command, &json!({"tracks": ids})).is_err());
    assert_eq!(serde_json::to_value(e.session()).unwrap(), before);
    assert_eq!(e.session().pool.len(), pool);
    assert_eq!(e.undo_history(), history);
    assert!(e.can_redo());
    assert_eq!(e.is_dirty(), dirty);
}

#[test]
fn bounce_rejects_literal_inverted_window_before_staging() {
    let f = Folder::new();
    let (e, _) = engine();
    assert!(start(&e, &f.output(), Range { start: 2, end: 1 }, None, false).is_err());
    assert!(f.empty());
}

#[test]
fn bounce_rejects_unrepresentable_window_before_staging() {
    let f = Folder::new();
    let (e, _) = engine();
    assert!(start(&e, &f.output(), Range { start: i64::MIN, end: i64::MAX }, None, false).is_err());
    assert!(f.empty());
}

#[test]
fn bounce_rejects_unknown_bus_before_staging() {
    let f = Folder::new();
    let (e, _) = engine();
    assert!(start(&e, &f.output(), Range::new(0, 32), Some(BusId(u64::MAX)), false).is_err());
    assert!(f.empty());
}

#[test]
fn bounce_rejects_unsupported_selected_layout_before_staging() {
    let f = Folder::new();
    let (mut e, _) = engine();
    let bus = e.session_mut().add_bus("Wide", ChannelFormat::Ambisonics(4));
    assert!(start(&e, &f.output(), Range::new(0, 32), Some(bus), false).is_err());
    assert!(f.empty());
    e.session_mut().outputs = vec![soundcraft_model::OutputPath { name: "Wide main".into(), first_channel: 0, format: ChannelFormat::Ambisonics(4) }];
    assert!(start(&e, &f.output(), Range::new(0, 32), None, false).is_err());
    assert!(f.empty());
}

#[test]
fn bounce_checks_destination_authority_before_window_diagnostics() {
    let allowed = Folder::new();
    let outside = Folder::new();
    let (e, _) = engine();
    let roots = AutomationRoots::new(None, Some(&allowed.0)).unwrap().unwrap();
    let error = confine(Some(&roots), || start(&e, &outside.output(), Range { start: 2, end: 1 }, None, false)).err().unwrap().to_string();
    assert!(error.contains("outside the write root"), "{error}");
    assert!(allowed.empty() && outside.empty());
}

#[test]
fn streamed_bounce_propagates_latency_extension_failure_in_both_modes() {
    let f = Folder::new();
    let (mut e, track) = engine();
    e.session_mut().track_mut(track).unwrap().mixer.inserts[0] = Some(Insert::new("maximizer"));
    for normalize in [false, true] {
        let job = start(&e, &f.output(), Range { start: i64::MAX - 2, end: i64::MAX }, None, normalize).unwrap();
        assert!(job.run().is_err(), "normalize={normalize}");
        assert!(f.empty(), "failed render must clean its owned output stage");
    }
}

#[test]
fn direct_commit_tail_overflow_preserves_document_and_source_visibility() {
    let mut s = Session::default();
    let track = add_track(&mut s, "Overflow", ChannelFormat::Mono, i64::MAX);
    let mut e = Engine::new(s);
    let before = e.session_arc();
    assert!(commit_track(&mut e, track).is_err());
    assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()));
    assert!(!e.session().track(track).unwrap().hidden);
    assert!(!e.session().track(track).unwrap().inactive);
}

#[test]
fn later_commit_target_budget_failure_preserves_document_and_history() {
    let mut s = Session::default();
    let first = add_track(&mut s, "Ordinary", ChannelFormat::Mono, 64);
    let long = add_track(&mut s, "Long", ChannelFormat::Stereo, (soundcraft_mix::MAX_RENDER_SAMPLES / 2 + 1) as i64);
    let mut e = Engine::new(s);
    unchanged_failure(&mut e, "track.commit", &[first, long]);
    assert!(!e.session().track(first).unwrap().hidden);
}

#[test]
fn later_commit_target_layout_failure_preserves_document_and_history() {
    let mut s = Session::default();
    let first = add_track(&mut s, "Ordinary", ChannelFormat::Mono, 64);
    let wide = add_track(&mut s, "Wide", ChannelFormat::Ambisonics(4), 64);
    let mut e = Engine::new(s);
    unchanged_failure(&mut e, "track.commit", &[first, wide]);
}

#[test]
fn track_bounce_alias_propagates_checked_commit_tail_failure() {
    let mut s = Session::default();
    let track = add_track(&mut s, "Overflow", ChannelFormat::Mono, i64::MAX);
    let mut e = Engine::new(s);
    unchanged_failure(&mut e, "track.bounce", &[track]);
}

#[test]
fn committing_an_empty_audio_track_remains_a_no_op() {
    let mut e = Engine::default();
    let track = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Empty"));
    let before = e.session_arc();
    assert_eq!(commit_track(&mut e, track).unwrap(), None);
    assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()));
}

#[test]
fn ordinary_commit_preserves_pre_fader_pcm_and_one_step_undo() {
    let (mut e, track) = engine();
    let end = 64 + e.session().sample_rate.samples(2.0);
    let expected = soundcraft_mix::render_track_pre_fader(e.session(), track, Range::new(0, end)).unwrap();
    let before = serde_json::to_value(e.session()).unwrap();
    let result = e.execute("track.commit", &json!({"tracks": [track.0]})).unwrap();
    let committed = TrackId(result["tracks"][0].as_u64().unwrap());
    let source = e.session().track(committed).unwrap().clips()[0].source().unwrap();
    assert_eq!(e.session().pool.get(source).unwrap().buffer.channels, expected);
    assert!(e.session().track(track).unwrap().hidden);
    assert!(e.session().track(track).unwrap().inactive);
    assert!(e.is_dirty());
    assert_eq!(e.undo_history().len(), 1);
    assert!(e.undo());
    assert_eq!(serde_json::to_value(e.session()).unwrap(), before);
}
