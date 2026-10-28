//! Video (picture) commands: Video track online/offline, picture offset, the movie list.
//!
//! Movies are imported with `file.import_video`; the session only references them
//! (`Session::videos`). Decoding happens in the UI (`soundcraft-video`), never in the engine.
use super::{CommandSpec, always, bad, position_param, track_param};
use soundcraft_time::MAX_POSITION;

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{ClipContent, TrackKind};

/// `edit.flags` key set while the Video track is offline (online is the default).
pub const OFFLINE_FLAG: &str = "video.offline";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "video.online", "Video Track Online", ["Options"], None, "{value?: bool} (toggles when omitted) — when offline the Video window and the Video track stop decoding pictures", always, |e, p| {
            let s = e.session_mut();
            let online = p.get("value").and_then(Value::as_bool).unwrap_or(s.edit.flag(OFFLINE_FLAG));
            s.edit.set_flag(OFFLINE_FLAG, !online);
            Ok(json!({"value": online}))
        }),
        cmd!(
            "video.offset",
            "Set Video Start",
            [],
            None,
            "{track?: video track (default: the first), at: position} — moves the track's video clips so the picture starts at `at` (e.g. '01:00:00:00')",
            always,
            offset
        ),
        cmd!(query "video.list", "Movies", [], None, "{} — movies used by Video tracks (path, size, rate, codec, online file, why it is offline)", always, |e, _| {
            let s = e.session();
            let rows: Vec<Value> = s
                .videos
                .iter()
                .map(|v| {
                    let clips = s.tracks.iter().flat_map(|t| t.clips()).filter(|c| c.video_source() == Some(v.id)).count();
                    json!({"id": v.id, "name": v.name, "path": v.path, "width": v.width, "height": v.height, "frame_rate": v.frame_rate,
                        "duration": v.duration, "codec": v.codec, "clips": clips,
                        "file_found": v.offline.is_none() && crate::io::file_found(&v.path), "offline": v.offline})
                })
                .collect();
            Ok(json!({"videos": rows, "online": !s.edit.flag(OFFLINE_FLAG)}))
        }),
    ]
}

fn offset(e: &mut Engine, p: &Value) -> Result<Value> {
    let at = position_param(e, "video.offset", p, "at")?.ok_or_else(|| bad("video.offset", "`at` required"))?.max(0);
    let track = match track_param(e, "video.offset", p, "track")? {
        Some(t) => t,
        None => e
            .session()
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Video)
            .map(|t| t.id)
            .ok_or_else(|| bad("video.offset", "the session has no Video track"))?,
    };
    let s = e.session_mut();
    let tr = s.track_mut(track).ok_or_else(|| bad("video.offset", "no such track"))?;
    if tr.kind != TrackKind::Video {
        return Err(bad("video.offset", format!("`{}` is not a Video track", tr.name)));
    }
    let Some(pl) = tr.playlist_mut() else { return Err(bad("video.offset", "the track has no playlist")) };
    let first = pl.clips.iter().filter(|c| matches!(c.content, ClipContent::Video { .. })).map(|c| c.start).min();
    let Some(first) = first else { return Err(bad("video.offset", "the Video track has no clips")) };
    let delta = at.saturating_sub(first);
    for c in &mut pl.clips {
        c.start = c.start.saturating_add(delta).clamp(0, MAX_POSITION);
    }
    pl.sort();
    Ok(json!({"track": track, "start": at, "moved_by": delta}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, Clip, ClipContent, SourceId, TrackKind, VideoSource};

    fn video_engine() -> (Engine, soundcraft_model::TrackId) {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Video, ChannelFormat::Mono, None);
        let vid = SourceId(s.alloc());
        s.videos.push(VideoSource {
            id: vid,
            name: "m.mov".into(),
            path: "/nonexistent/m.mov".into(),
            width: 64,
            height: 48,
            frame_rate: 24.0,
            duration: 2.0,
            codec: "h264".into(),
            offline: None,
        });
        let c = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::video(c, "m", vid, 0, 1000, 96_000));
        (e, t)
    }

    #[test]
    fn offset_moves_the_picture_and_undoes() {
        let (mut e, t) = video_engine();
        e.execute("video.offset", &json!({"at": 48_000})).unwrap();
        assert_eq!(e.session().track(t).unwrap().clips()[0].start, 48_000);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(t).unwrap().clips()[0].start, 1000);
        assert!(e.execute("video.offset", &json!({})).is_err());
        let a = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("A"));
        assert!(e.execute("video.offset", &json!({"track": a.0, "at": 0})).is_err());
        assert!(e.execute("video.offset", &json!({"at": i64::MAX})).is_ok());
    }

    #[test]
    fn online_toggles_and_list_reports_missing_files() {
        let (mut e, _) = video_engine();
        assert_eq!(e.execute("video.online", &json!({})).unwrap()["value"], false);
        assert_eq!(e.execute("video.online", &json!({})).unwrap()["value"], true);
        assert_eq!(e.execute("video.online", &json!({"value": false})).unwrap()["value"], false);
        let l = e.execute("video.list", &json!({})).unwrap();
        assert_eq!(l["online"], false);
        assert_eq!(l["videos"][0]["file_found"], false);
        assert_eq!(l["videos"][0]["clips"], 1);
    }

    #[test]
    fn video_clips_are_silent_in_the_mix() {
        let (mut e, t) = video_engine();
        assert!(matches!(e.session().track(t).unwrap().clips()[0].content, ClipContent::Video { .. }));
        e.session_mut().add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        let dir = std::env::temp_dir().join(format!("sc-video-bounce-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("mix.wav");
        e.execute("file.bounce_mix", &json!({"path": out.to_string_lossy(), "start": 0, "end": 96_000, "dither": false})).unwrap();
        let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(&out).unwrap(), Some("wav")).unwrap();
        assert!(buf.channels.iter().flatten().all(|x| *x == 0.0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn import_video_adds_a_video_track_and_survives_save_and_open() {
        let dir = std::env::temp_dir().join(format!("sc-video-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let movie = dir.join("picture.mp4");
        std::fs::write(&movie, soundcraft_video::synth::h264_mp4(30, 10).unwrap()).unwrap();
        let mut e = Engine::default();
        let r = e.execute("file.import_video", &json!({"path": movie.to_string_lossy(), "at": 4800})).unwrap();
        assert!(r["audio"].is_null() && r["audio_error"].is_string(), "{r}");
        let s = e.session();
        assert_eq!(s.tracks[0].kind, TrackKind::Video);
        let clip = s.tracks[0].clips()[0].clone();
        assert_eq!((clip.start, clip.length), (4800, 60_000));
        let v = s.video(clip.video_source().unwrap()).unwrap().clone();
        assert_eq!((v.width, v.height, v.codec.as_str()), (64, 48, "h264"));
        assert!((v.frame_rate - 24.0).abs() < 1e-9);
        // A second import does not overlap: it goes onto the same Video track after the first.
        e.execute("file.import_video", &json!({"path": movie.to_string_lossy(), "at": 96_000})).unwrap();
        assert_eq!(e.session().tracks.iter().filter(|t| t.kind == TrackKind::Video).count(), 1);
        e.execute("edit.undo", &json!({})).unwrap();
        // Save, reopen: the clip and the movie reference round-trip; nothing is missing.
        let sess = dir.join("s.scraft");
        e.execute("session.save_as", &json!({"path": sess.to_string_lossy()})).unwrap();
        let mut e2 = Engine::default();
        let missing = crate::io::open_session(&mut e2, &sess).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(e2.session().tracks[0].clips()[0], clip);
        assert_eq!(e2.session().videos, e.session().videos);
        // Movie moved next to the session's "Video Files" folder: relinked on open.
        std::fs::create_dir_all(dir.join("Video Files")).unwrap();
        std::fs::rename(&movie, dir.join("Video Files").join("picture.mp4")).unwrap();
        let mut e3 = Engine::default();
        assert!(crate::io::open_session(&mut e3, &sess).unwrap().is_empty());
        assert!(e3.session().videos[0].path.contains("Video Files"));
        // Movie gone: the session still opens and reports it.
        std::fs::remove_file(dir.join("Video Files").join("picture.mp4")).unwrap();
        let mut e4 = Engine::default();
        let missing = crate::io::open_session(&mut e4, &sess).unwrap();
        assert!(missing.iter().any(|m| m.contains("picture.mp4")), "{missing:?}");
        assert!(e4.messages.iter().any(|m| m.contains("missing media")));
        assert_eq!(e4.session().tracks[0].clips().len(), 1);
        // Neither picture nor audio: an error, and nothing changes.
        let junk = dir.join("junk.mov");
        std::fs::write(&junk, b"not a movie at all").unwrap();
        let before = e4.session().tracks.len();
        assert!(e4.execute("file.import_video", &json!({"path": junk.to_string_lossy()})).is_err());
        assert_eq!(e4.session().tracks.len(), before);
        let _ = std::fs::remove_dir_all(dir);
    }
}
