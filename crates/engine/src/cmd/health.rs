//! Media availability checks without decoding files or changing the session.
use super::{CommandSpec, always};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::ClipContent;
use std::collections::{BTreeMap, HashSet};

/// Registers read-only session media diagnostics.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "session.audio_health", "Session Media Health", [], None, "{}; checks all audio clips, including inactive playlists, and referenced movies; no signal analysis", always, report),
        cmd!(query "session.media_status", "Session Media Status", [], None, "{}; reports whether active-playlist and complete-session media are online", always, |e, _| {
            let (timeline, session) = crate::inspect::media_online(e.session());
            Ok(json!({"timeline_online": timeline, "session_online": session}))
        }),
    ]
}

struct ClipContext<'a> {
    track: &'a soundcraft_model::Track,
    playlist: &'a soundcraft_model::Playlist,
    active: bool,
    session_sample_rate: u32,
}

/// Adds every media or bounds problem found for one audio clip.
fn check_audio_clip(
    ctx: &ClipContext<'_>,
    clip: &soundcraft_model::Clip,
    source: soundcraft_model::SourceId,
    offset: i64,
    metadata: Option<&soundcraft_model::Source>,
    audio: Option<&soundcraft_model::SourceAudio>,
    issues: &mut Vec<Value>,
) {
    let mut add = |code: &str, message: &str| {
        issues.push(json!({"code": code, "message": message, "track": ctx.track.name, "track_id": ctx.track.id.0,
            "playlist": ctx.playlist.name, "active": ctx.active, "clip": clip.name, "clip_id": clip.id.0,
            "source_id": source.0, "path": metadata.map(|m| m.path.as_str())}));
    };
    if metadata.is_none() {
        add("missing_source", "Source metadata is missing. Reimport the audio and replace this clip.");
    }
    let Some(audio) = audio else {
        add("unavailable_audio", "Audio is not loaded; this clip will be silent. Restore the media and reopen the session, or reimport it.");
        return;
    };
    if audio.buffer.sample_rate != ctx.session_sample_rate {
        add("sample_rate_mismatch", "Loaded audio differs from the session sample rate. Reimport it to convert to the session rate.");
    }
    if audio.buffer.channels.is_empty() || audio.frames() == 0 {
        add("empty_audio", "Loaded audio has no playable frames. Reimport a valid audio file.");
        return;
    }
    if audio.buffer.channels.iter().any(|channel| channel.len() != audio.frames()) {
        add("uneven_channels", "Loaded audio channels have different lengths. Reimport a valid audio file; bounds below use the shortest channel.");
    }
    if !clip.stretch.is_finite() || clip.stretch <= 0.0 || clip.length <= 0 {
        add("invalid_clip", "Clip length or stretch is invalid. Replace this clip.");
    }
    let stretch = if clip.stretch.is_finite() && clip.stretch > 0.0 { clip.stretch } else { 1.0 };
    // Match the mixer's last requested source sample, including elastic playback.
    let last = offset as f64 + clip.length.saturating_sub(1).max(0) as f64 / stretch;
    if offset < 0 || last >= audio.frames() as f64 {
        add("source_bounds", "Clip extends outside the loaded audio; part will be silent. Trim the clip or replace its source.");
    }
}

/// Builds a read-only health report for audio clips and referenced movies.
fn report(e: &mut Engine, _: &Value) -> Result<Value> {
    let s = e.session();
    let sources: BTreeMap<_, _> = s.sources.iter().map(|source| (source.id, source)).collect();
    let mut issues = Vec::new();
    if s.sample_rate.hz() == 0 {
        issues.push(json!({"code": "invalid_sample_rate", "track": "Session", "message": "Session sample rate is zero. Set a valid rate in Session Setup before playback.", "active": true}));
    }
    let mut clips = 0usize;
    let mut active_end = 0i64;
    let mut referenced_audio = HashSet::new();
    for track in &s.tracks {
        for (index, playlist) in track.playlists.iter().enumerate() {
            let active = index == track.active_playlist;
            let ctx = ClipContext { track, playlist, active, session_sample_rate: s.sample_rate.hz() };
            for clip in &playlist.clips {
                let ClipContent::Audio { source, offset } = clip.content else { continue };
                referenced_audio.insert(source);
                clips += 1;
                if active {
                    active_end = active_end.max(clip.end());
                }
                let metadata = sources.get(&source).copied();
                let audio = s.pool.get(source).map(|a| a.as_ref());
                check_audio_clip(&ctx, clip, source, offset, metadata, audio, &mut issues);
            }
        }
    }
    for source in s.sources.iter().filter(|source| !referenced_audio.contains(&source.id)) {
        if !crate::inspect::audio_source_online(s, source.id) {
            issues.push(json!({
                "code": "unavailable_audio_source",
                "message": "An audio source kept by the session is not loaded or playable. Restore or remove the source.",
                "track": "Session",
                "playlist": "",
                "active": false,
                "clip": "",
                "source_id": source.id.0,
                "path": source.path,
            }));
        }
    }
    for source in &s.videos {
        if !crate::inspect::video_source_online(source) {
            let message = source.offline.as_deref().unwrap_or("Movie file is missing or unavailable. Relink the movie before playback.");
            issues.push(json!({
                "code": "unavailable_video",
                "message": message,
                "track": "Video",
                "playlist": "",
                "active": true,
                "clip": source.name,
                "source_id": source.id.0,
                "path": source.path,
            }));
        }
    }
    Ok(json!({"audio_clips": clips, "video_sources": s.videos.len(), "active_audio_end_samples": active_end,
        "active_audio_end_seconds": (s.sample_rate.hz() != 0).then(|| active_end as f64 / f64::from(s.sample_rate.hz())),
        "issue_count": issues.len(), "healthy": issues.is_empty(), "issues": issues,
        "scope": "All playlists and referenced movies; loaded audio availability, clip bounds and movie file availability. Does not check signal levels, plugins or routing."}))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{ChannelFormat, Clip, ClipId, SourceAudio, SourceId, TrackKind, VideoSource};
    use std::sync::Arc;

    fn check(e: &mut Engine) -> Value {
        e.execute("session.audio_health", &json!({})).unwrap()
    }

    #[test]
    fn empty_and_demo_are_healthy_and_query_is_read_only() {
        for mut e in [Engine::default(), crate::demo::demo_engine()] {
            let before = e.session_arc();
            let revision = e.revision;
            let dirty = e.is_dirty();
            assert_eq!(check(&mut e)["healthy"], true);
            assert_eq!(*before, *e.session());
            assert_eq!(e.revision, revision);
            assert_eq!(e.is_dirty(), dirty);
            assert!(e.journal.is_empty());
            assert!(!e.can_undo());
        }
    }

    /// Active-playlist and complete-session availability are reported separately.
    #[test]
    fn media_status_distinguishes_active_and_alternate_playlists() {
        let mut e = crate::demo::demo_engine();
        let id = e.session().sources[0].id;
        assert!(e.session_mut().pool.remove(id).is_some());
        let status = e.execute("session.media_status", &json!({})).unwrap();
        assert_eq!(status["session_online"], false);
        let expected_timeline = e.session().tracks.iter().flat_map(|track| track.clips()).all(|clip| clip.source() != Some(id));
        assert_eq!(status["timeline_online"], expected_timeline);
    }

    /// An orphaned source that status marks offline also appears in the diagnostic report.
    #[test]
    fn health_report_explains_unplayable_unreferenced_audio_sources() {
        let mut e = crate::demo::demo_engine();
        let mut source = e.session().sources[0].clone();
        source.id = SourceId(9_999);
        source.name = "Orphaned source".into();
        e.session_mut().sources.push(source);
        let status = e.execute("session.media_status", &json!({})).unwrap();
        assert_eq!(status["timeline_online"], true);
        assert_eq!(status["session_online"], false);
        let report = check(&mut e);
        assert_eq!(report["healthy"], false);
        assert!(
            report["issues"]
                .as_array()
                .is_some_and(|issues| { issues.iter().any(|issue| issue["code"] == "unavailable_audio_source" && issue["source_id"] == 9_999) })
        );
    }

    /// Missing and explicitly offline movies appear in both status and health reports.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn media_status_checks_referenced_video_files() {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("soundcraft-media-status-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("picture.mov");
        std::fs::write(&path, b"movie").unwrap();
        let mut e = Engine::default();
        let source = SourceId(900);
        let track = e.session_mut().add_track(TrackKind::Video, ChannelFormat::Mono, None);
        let clip = e.session_mut().new_clip_id();
        e.session_mut().videos.push(VideoSource {
            id: source,
            name: "Picture".into(),
            path: path.to_string_lossy().into_owned(),
            width: 1,
            height: 1,
            frame_rate: 24.0,
            duration: 1.0,
            codec: "test".into(),
            offline: None,
        });
        e.session_mut().track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::video(clip, "Picture", source, 0, 0, 48_000));
        assert_eq!(e.execute("session.media_status", &json!({})).unwrap()["timeline_online"], true);
        e.session_mut().videos[0].offline = Some("Movie path was refused".into());
        assert_eq!(e.execute("session.media_status", &json!({})).unwrap()["timeline_online"], false);
        assert_eq!(check(&mut e)["issues"][0]["message"], "Movie path was refused");
        e.session_mut().videos[0].offline = None;
        std::fs::remove_file(&path).unwrap();
        let status = e.execute("session.media_status", &json!({})).unwrap();
        assert_eq!(status["timeline_online"], false);
        assert_eq!(status["session_online"], false);
        let report = check(&mut e);
        assert_eq!(report["healthy"], false);
        assert_eq!(report["video_sources"], 1);
        assert_eq!(report["issues"][0]["code"], "unavailable_video");
        assert_eq!(report["issues"][0]["path"], path.to_string_lossy().as_ref());
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn reports_missing_media_in_inactive_takes() {
        let mut e = crate::demo::demo_engine();
        let t = e.session_mut().tracks.first_mut().unwrap();
        let mut p = soundcraft_model::Playlist::new("Alternate take");
        p.clips.push(Clip::audio(ClipId(999), "Lost take", SourceId(999), 0, 0, 10));
        t.playlists.push(p);
        let r = check(&mut e);
        assert_eq!(r["issue_count"], 2);
        assert_eq!(r["issues"][0]["code"], "missing_source");
        assert_eq!(r["issues"][1]["code"], "unavailable_audio");
        assert_eq!(r["issues"][0]["active"], false);
    }

    #[test]
    fn checks_loaded_bounds_and_stretch_not_stale_metadata() {
        let mut e = crate::demo::demo_engine();
        let id = e.session().sources.first().unwrap().id;
        let sr = e.session().sample_rate.hz();
        e.session_mut().pool.insert(id, Arc::new(SourceAudio::new(AudioBuffer::new(sr, 1, 10))));
        let t = e.session_mut().tracks.first_mut().unwrap();
        t.playlists.clear();
        let mut p = soundcraft_model::Playlist::new("Bounds");
        let mut c = Clip::audio(ClipId(999), "Stretch", id, 0, 0, 20);
        c.stretch = 2.0;
        p.clips.push(c);
        t.playlists.push(p);
        t.active_playlist = 0;
        assert_eq!(check(&mut e)["healthy"], true);
        e.session_mut().tracks.first_mut().unwrap().playlists[0].clips[0].length = 21;
        assert_eq!(check(&mut e)["issues"][0]["code"], "source_bounds");
        let clip = &mut e.session_mut().tracks.first_mut().unwrap().playlists[0].clips[0];
        clip.length = i64::MAX;
        clip.stretch = f64::NAN;
        clip.content = ClipContent::Audio { source: id, offset: i64::MAX };
        let r = check(&mut e);
        assert_eq!(r["issues"][0]["code"], "invalid_clip");
        assert_eq!(r["issues"][1]["code"], "source_bounds");
    }

    #[test]
    fn malformed_and_empty_audio_are_reported_without_panicking() {
        let mut e = crate::demo::demo_engine();
        let id = e.session().sources.first().unwrap().id;
        e.session_mut().pool.insert(id, Arc::new(SourceAudio::new(AudioBuffer::new(1, 0, 0))));
        let r = check(&mut e);
        assert!(r["issues"].as_array().unwrap().iter().any(|i| i["code"] == "empty_audio"));
        assert!(r["issues"].as_array().unwrap().iter().any(|i| i["code"] == "sample_rate_mismatch"));
    }

    #[test]
    fn zero_rate_has_unknown_duration_and_uneven_channels_use_shortest() {
        let mut e = crate::demo::demo_engine();
        let id = e.session().sources.first().unwrap().id;
        let sr = e.session().sample_rate.hz();
        let audio = AudioBuffer { sample_rate: sr, channels: vec![vec![0.0; 10], vec![0.0; 5]] };
        e.session_mut().pool.insert(id, Arc::new(SourceAudio::new(audio)));
        let r = check(&mut e);
        assert!(r["issues"].as_array().unwrap().iter().any(|i| i["code"] == "uneven_channels"));
        assert!(r["issues"].as_array().unwrap().iter().any(|i| i["code"] == "source_bounds"));
        e.session_mut().sample_rate = serde_json::from_value(json!(0)).unwrap();
        let r = check(&mut e);
        assert!(r["active_audio_end_seconds"].is_null());
        assert_eq!(r["issues"][0]["code"], "invalid_sample_rate");
    }
}
