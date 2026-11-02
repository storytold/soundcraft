use super::import::add_source;
#[cfg(test)]
use super::session::{save_copy, save_session};
use super::{Clip, Engine, EngineError, FileFormat, OsStr, PathBuf, Range, Samples, TrackId, read_file};

/// A recording's plan, made when it starts: each record-enabled track's take file, where the
/// takes start on the timeline, the loop length they split into when loop recording, and the
/// punch range they're trimmed to.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Recording {
    pub start: Samples,
    pub pass: Option<Samples>,
    pub punch: Option<Range>,
    /// The input's sample rate, which the take files are written at.
    pub rate: u32,
    pub takes: Vec<Take>,
}

/// One track's take file.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Take {
    pub track: TrackId,
    pub path: PathBuf,
    pub channels: usize,
}

/// Place recorded take files as `plan` says, as one undo step ("Record").
pub fn place_recording(e: &mut Engine, plan: &Recording, takes: Vec<(TrackId, PathBuf)>) -> Vec<soundcraft_model::ClipId> {
    add_recording_in(e, plan.start, takes, plan.pass, plan.punch)
}

/// Turn recorded take files into clips at `start`, as one undo step ("Record"); each file is the
/// source of its clips. With `pass` (loop recording), a take becomes one clip per pass (at most
/// 1000), each pass after the first on a new playlist.
pub fn add_recording(e: &mut Engine, start: Samples, takes: Vec<(TrackId, PathBuf)>, pass: Option<Samples>) -> Vec<soundcraft_model::ClipId> {
    add_recording_in(e, start, takes, pass, None)
}

/// [`add_recording`] limited to the `punch` range: each clip is cut to it before it's placed, so
/// the material around the punch isn't overwritten.
fn add_recording_in(
    e: &mut Engine,
    start: Samples,
    takes: Vec<(TrackId, PathBuf)>,
    pass: Option<Samples>,
    punch: Option<Range>,
) -> Vec<soundcraft_model::ClipId> {
    let before = e.session_arc();
    let mut out = Vec::new();
    for (tid, path) in takes {
        let read =
            read_file(&path).and_then(|b| soundcraft_audio_io::decode(&b, None).map_err(|err| EngineError::Io(format!("{}: {err}", path.display()))));
        let Ok((_, buf)) = read.inspect_err(|err| e.message(err.to_string())) else { continue };
        let s = e.session_mut();
        if s.track(tid).is_none() {
            continue;
        }
        let hz = s.sample_rate.hz();
        let resampled = buf.sample_rate != hz;
        let mut buf = buf;
        if resampled && buf.sample_rate > 0 {
            buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, hz);
            buf.sample_rate = hz;
        }
        let name = path.file_stem().and_then(OsStr::to_str).unwrap_or("Take");
        let len = i64::try_from(buf.frames()).unwrap_or(0);
        let pass = pass.unwrap_or(len).max(1);
        // Work out what each pass keeps first, so a take nothing is placed from isn't registered.
        let mut kept_passes = Vec::new();
        for offset in (0..len).step_by(usize::try_from(pass).unwrap_or(usize::MAX)).take(1_000) {
            let n = pass.min(len - offset);
            if offset > 0 && n < pass / 8 {
                break;
            }
            let (clip_start, cut, clip_len) = match punch {
                Some(p) => {
                    let Some(kept) = Range::new(start, start.saturating_add(n)).intersect(&p) else { continue };
                    (kept.start, kept.start.saturating_sub(start), kept.len())
                }
                None => (start, 0, n),
            };
            kept_passes.push((offset, clip_start, cut, clip_len));
        }
        if kept_passes.is_empty() {
            continue;
        }
        let src = add_source(s, name, buf, Some(&path.to_string_lossy()), FileFormat::Wav);
        if let Some(x) = s.sources.last_mut() {
            x.unsaved = resampled;
        }
        for (offset, clip_start, cut, clip_len) in kept_passes {
            if offset > 0
                && let Some(t) = s.track_mut(tid)
            {
                t.playlists.push(soundcraft_model::Playlist::new(format!("{}.{:02}", t.name, t.playlists.len() + 1)));
                t.active_playlist = t.playlists.len() - 1;
            }
            let cid = s.new_clip_id();
            crate::edit::place_clip(s, tid, Clip::audio(cid, name, src, offset.saturating_add(cut), clip_start, clip_len));
            out.push(cid);
        }
    }
    if !out.is_empty() {
        e.push_undo("Record", before);
    }
    out
}

/// Where a take recording from `start` shows while it holds `len` samples, as it will be placed:
/// just the pass under way when loop recording in passes of `pass`, within the `punch` range.
/// Returns the timeline position of its first sample and the span it covers.
pub fn live_span(start: Samples, pass: Option<Samples>, punch: Option<Range>, len: Samples) -> Option<(Samples, Range)> {
    let origin = start.saturating_sub(pass.filter(|p| *p > 0).map_or(0, |p| len / p * p));
    let span = Range::new(start, origin.saturating_add(len));
    Some((origin, span.intersect(&punch.unwrap_or(span))?))
}

#[cfg(test)]
mod record_tests {
    use super::super::{AudioBuffer, Path, plan_takes};
    use super::*;
    use serde_json::json;

    fn armed(e: &mut Engine, name: &str, format: &str, input: &str) -> TrackId {
        let id = e.execute("track.new", &json!({"name": name, "format": format})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [id], "input": input})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
        TrackId(id)
    }

    fn write_take(path: &Path, frames: usize) {
        let mut bytes = soundcraft_audio_io::wav_float_header(1, 48_000, frames as u64).unwrap();
        bytes.extend((0..frames).flat_map(|i| (i as f32 / frames as f32 - 0.5).to_le_bytes()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn armed_tracks_get_new_take_files_used_in_place() {
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 2");
        let gtr = armed(&mut e, "Gtr", "stereo", "In 3-4");
        write_take(&dir.join("Audio Files/Vox_01.wav"), 10);
        let plan = plan_takes(&e, &dir, 4);
        assert_eq!(plan, vec![(vox, vec![1], dir.join("Audio Files/Vox_02.wav")), (gtr, vec![2, 3], dir.join("Audio Files/Gtr_01.wav"))]);
        assert_eq!(plan_takes(&e, &dir, 2)[1].1, vec![0, 1]);

        let session = dir.join("Song.scraft").to_string_lossy().into_owned();
        e.execute("session.save_as", &json!({"path": session})).unwrap();
        write_take(&plan[0].2, 4_800);
        let clips = add_recording(&mut e, 96_000, vec![(vox, plan[0].2.clone())], None);
        let clip = e.session().find_clip(clips[0]).unwrap().1.clone();
        assert_eq!((clip.start, clip.length), (96_000, 4_800));
        assert_eq!(save_session(&mut e, &session).unwrap(), 0);
        let src = e.session().sources.last().unwrap().clone();
        assert_eq!((src.name.as_str(), src.path.as_str(), src.unsaved, src.frames), ("Vox_02", "Audio Files/Vox_02.wav", false, 4_800));
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(clips[0]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_loop_take_becomes_one_clip_per_pass_and_autosaves_in_place() {
        let dir = std::env::temp_dir().join(format!("soundcraft-loop-take-{}", std::process::id()));
        let path = dir.join("Audio Files/Vox_01.wav");
        write_take(&path, 1_050);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        assert_eq!(add_recording(&mut e, 1_000, vec![(vox, path.clone())], Some(400)).len(), 3);
        assert_eq!(save_copy(&e, &dir.join("Untitled.scraft").to_string_lossy()).unwrap(), 0);
        let t = e.session().track(vox).unwrap();
        let passes: Vec<_> = t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.source_offset(), c.start, c.length))).collect();
        assert_eq!(passes, vec![(0, 1_000, 400), (400, 1_000, 400), (800, 1_000, 250)]);
        assert_eq!((t.active_playlist, t.playlists[2].name.as_str()), (2, "Vox.03"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn punching_in_keeps_the_material_around_the_punch_range() {
        let dir = std::env::temp_dir().join(format!("soundcraft-punch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        let base = dir.join("Audio Files/Base.wav");
        let take = dir.join("Audio Files/Take.wav");
        write_take(&base, 3_000);
        write_take(&take, 1_000);
        add_recording(&mut e, 0, vec![(vox, base)], None);
        let plan = Recording {
            start: 1_000,
            pass: None,
            punch: Some(Range::new(1_400, 1_600)),
            rate: 48_000,
            takes: vec![Take { track: vox, path: take.clone(), channels: 1 }],
        };
        place_recording(&mut e, &plan, vec![(vox, take)]);
        let clips: Vec<_> = e.session().track(vox).unwrap().clips().iter().map(|c| (c.start, c.length, c.source_offset())).collect();
        assert_eq!(clips, vec![(0, 1_400, 0), (1_400, 200, 400), (1_600, 1_400, 1_600)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_punch_that_misses_the_take_leaves_the_session_alone() {
        let dir = std::env::temp_dir().join(format!("soundcraft-punch-miss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        let take = dir.join("Audio Files/Take.wav");
        write_take(&take, 1_000);
        let shared = add_source(
            e.session_mut(),
            "Take",
            AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 100]] },
            Some(&take.to_string_lossy()),
            FileFormat::Wav,
        );
        let sources = e.session().sources.len();
        let pool = e.session().pool.len();
        let plan = Recording {
            start: 1_000,
            pass: None,
            punch: Some(Range::new(5_000, 6_000)),
            rate: 48_000,
            takes: vec![Take { track: vox, path: take.clone(), channels: 1 }],
        };
        let placed = place_recording(&mut e, &plan, vec![(vox, take)]);
        assert!(placed.is_empty());
        assert_eq!(e.session().sources.len(), sources);
        assert_eq!(e.session().pool.len(), pool);
        assert!(e.session().sources.iter().any(|x| x.id == shared));
        assert!(e.session().pool.contains(shared));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_take_shows_the_pass_under_way_within_the_punch_while_it_records() {
        assert_eq!(live_span(1_000, None, None, 0), None);
        assert_eq!(live_span(1_000, None, None, 500), Some((1_000, Range::new(1_000, 1_500))));
        assert_eq!(live_span(1_000, Some(400), None, 900), Some((200, Range::new(1_000, 1_100))));
        assert_eq!(live_span(1_000, Some(400), None, 800), None);
        let punch = Some(Range::new(1_200, 1_400));
        assert_eq!(live_span(1_000, None, punch, 150), None);
        assert_eq!(live_span(1_000, None, punch, 500), Some((1_000, Range::new(1_200, 1_400))));
    }
}
