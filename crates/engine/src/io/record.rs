use super::import::add_source;
#[cfg(test)]
use super::session::{save_copy, save_session};
use super::{Clip, Engine, EngineError, FileFormat, OsStr, PathBuf, Range, Samples, TrackId, read_file};

/// A recording's plan, made when it starts: each record-enabled track's take file, where the
/// capture starts on the timeline, the loop length they split into when loop recording, and the
/// selected range. The range trims a punch or locates the first loop after pre-roll.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Recording {
    pub start: Samples,
    pub pass: Option<Samples>,
    /// Selected punch or loop range, stored as `punch` in autosave plans.
    pub punch: Option<Range>,
    /// Measured device and callback delay in session samples. Older recovery plans start at zero correction.
    #[serde(default)]
    pub placement_offset: Samples,
    /// The input's sample rate, which the take files are written at.
    pub rate: u32,
    pub takes: Vec<Take>,
}

impl Recording {
    pub fn origin(&self) -> Samples {
        self.start.saturating_sub(self.placement_offset)
    }

    /// Keep leading capture from replacing material before the requested start.
    pub fn crop(&self) -> Option<Range> {
        self.punch.or_else(|| (self.placement_offset > 0).then(|| Range::new(self.start, soundcraft_time::MAX_POSITION)))
    }
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
    add_recording_in(e, plan.origin(), takes, plan.pass, plan.crop())
}

/// Turn recorded take files into clips at `start`, as one undo step ("Record"); each file is the
/// source of its clips. With `pass` (loop recording), a take becomes one clip per pass (at most
/// 1000), each pass after the first on a new playlist.
pub fn add_recording(e: &mut Engine, start: Samples, takes: Vec<(TrackId, PathBuf)>, pass: Option<Samples>) -> Vec<soundcraft_model::ClipId> {
    add_recording_in(e, start, takes, pass, None)
}

/// [`add_recording`] with an optional selection for punch or loop recording.
fn add_recording_in(
    e: &mut Engine,
    start: Samples,
    takes: Vec<(TrackId, PathBuf)>,
    pass: Option<Samples>,
    punch: Option<Range>,
) -> Vec<soundcraft_model::ClipId> {
    let before = e.session_arc();
    let mut out = Vec::new();
    let mut registered_source = false;
    for (tid, path) in takes {
        let read = read_file(&path).and_then(|b| craft_audio::decode(&b, None).map_err(|err| EngineError::Io(format!("{}: {err}", path.display()))));
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
        // Keep short loop takes reachable in the Clips list, even when no pass is placed.
        let kept_passes = recording_clips(start, len, pass, punch);
        if kept_passes.is_empty() && (len == 0 || pass.is_none_or(|samples| samples <= 0)) {
            continue;
        }
        let src = add_source(s, name, buf, Some(&path.to_string_lossy()), FileFormat::Wav);
        registered_source = true;
        if let Some(x) = s.sources.last_mut() {
            x.unsaved = resampled;
        }
        for (index, offset, clip_start, clip_len) in kept_passes {
            if index > 0
                && let Some(t) = s.track_mut(tid)
            {
                t.playlists.push(soundcraft_model::Playlist::new(format!("{}.{:02}", t.name, t.playlists.len() + 1)));
                t.active_playlist = t.playlists.len() - 1;
            }
            let cid = s.new_clip_id();
            crate::edit::place_clip(s, tid, Clip::audio(cid, name, src, offset, clip_start, clip_len));
            out.push(cid);
        }
    }
    if registered_source {
        e.push_undo("Record", before);
    }
    out
}

/// The pass index, offset in the source, timeline start, and length of each retained clip.
fn recording_clips(origin: Samples, len: Samples, pass: Option<Samples>, selection: Option<Range>) -> Vec<(Samples, Samples, Samples, Samples)> {
    let audio = Range::new(origin, origin.saturating_add(len));
    let Some(pass) = pass.filter(|p| *p > 0) else {
        let range = selection.unwrap_or_else(|| Range::new(0, soundcraft_time::MAX_POSITION));
        return audio.intersect(&range).map(|kept| (0, kept.start.saturating_sub(origin), kept.start, kept.len())).into_iter().collect();
    };
    let start = selection.map_or(origin, |range| range.start);
    let mut clips = Vec::new();
    for index in 0..1_000 {
        let Some(elapsed) = pass.checked_mul(index) else { break };
        let from = start.saturating_add(elapsed);
        // A partial final pass is kept only past the midpoint. The source WAV retains all audio.
        if audio.end.saturating_sub(from) <= pass / 2 {
            break;
        }
        if let Some(kept) = audio.intersect(&Range::new(from, from.saturating_add(pass))) {
            let clip_start = kept.start.saturating_sub(elapsed);
            let trim = clip_start.saturating_neg().max(0);
            if kept.len() > trim {
                clips.push((index, kept.start.saturating_sub(origin).saturating_add(trim), clip_start.max(0), kept.len() - trim));
            }
        }
    }
    clips
}

/// Where a take recording from `start` shows while it holds `len` samples, as it will be placed:
/// just the pass under way when loop recording in passes of `pass`, within the `punch` range.
/// Returns the timeline position of its first sample and the span it covers.
pub fn live_span(start: Samples, pass: Option<Samples>, punch: Option<Range>, len: Samples) -> Option<(Samples, Range)> {
    let loop_start = punch.map_or(start, |range| range.start);
    let elapsed = start.saturating_add(len).saturating_sub(loop_start).max(0);
    let wrapped = pass.filter(|p| *p > 0).map_or(0, |p| elapsed / p * p);
    let origin = start.saturating_sub(wrapped);
    let span = Range::new(origin, origin.saturating_add(len));
    let range = punch.unwrap_or_else(|| Range::new(if pass.is_some() { loop_start.max(0) } else { 0 }, span.end.max(0)));
    Some((origin, span.intersect(&range)?))
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
        let mut bytes = craft_audio::wav_float_header(48_000, 1, frames as u64).unwrap();
        bytes.extend((0..frames).flat_map(|i| (i as f32 / frames as f32 - 0.5).to_le_bytes()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn named_input_remaps_the_complete_recording_range_and_never_wraps() {
        let mut e = Engine::default();
        let dir = Path::new("unused");
        let track = armed(&mut e, "Keys", "stereo", "In 1-2");
        e.execute("setup.io", &json!({"action": "create_input", "name": "Keys pair", "format": "stereo", "first_channel": 30})).unwrap();
        assert!(e.is_dirty());
        e.execute("track.input", &json!({"track": track.0, "input": "keys pair"})).unwrap();
        assert_eq!(plan_takes(&e, dir, 32)[0].1, [30, 31]);
        assert!(plan_takes(&e, dir, 31).is_empty());
        e.execute("setup.io", &json!({"action": "rename_input", "name": "Keys pair", "new_name": "Piano"})).unwrap();
        e.execute("setup.io", &json!({"action": "set_input", "name": "Piano", "first_channel": 62})).unwrap();
        assert_eq!(e.session().track(track).unwrap().mixer.input, soundcraft_model::Route::Input("Piano".into()));
        assert_eq!(plan_takes(&e, dir, 64)[0].1, [62, 63]);
        assert!(plan_takes(&e, dir, 32).is_empty());
        e.execute("setup.io", &json!({"action": "delete_input", "name": "Piano"})).unwrap();
        assert!(plan_takes(&e, dir, 64).is_empty());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(plan_takes(&e, dir, 64)[0].1, [62, 63]);
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
    fn a_short_first_loop_take_remains_available_and_undoable() {
        let dir = std::env::temp_dir().join(format!("soundcraft-short-loop-{}", std::process::id()));
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        let take = dir.join("Audio Files/Short Loop.wav");
        write_take(&take, 499);
        let plan = Recording { start: 700, pass: Some(400), punch: Some(Range::new(1_000, 1_400)), placement_offset: 0, rate: 48_000, takes: vec![] };
        assert!(place_recording(&mut e, &plan, vec![(vox, take)]).is_empty());
        assert_eq!(e.session().sources.len(), 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().sources.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loop_passes_start_after_preroll_and_capture_offset_and_keep_only_a_majority_tail() {
        let selection = Some(Range::new(1_000, 1_400));
        let clips = |len| recording_clips(700, len, Some(400), selection);
        assert!(clips(499).is_empty(), "an early stop on the first pass places no clip");
        assert!(clips(500).is_empty(), "at the midpoint no majority was recorded");
        assert_eq!(clips(501), vec![(0, 300, 1_000, 201)]);
        let full = vec![(0, 300, 1_000, 400), (1, 700, 1_000, 400), (2, 1_100, 1_000, 400)];
        assert_eq!(clips(1_700), full);
        let mut with_tail = full;
        with_tail.push((3, 1_500, 1_000, 201));
        assert_eq!(clips(1_701), with_tail);
        assert_eq!(live_span(700, Some(400), selection, 200), None);
        assert_eq!(live_span(700, Some(400), selection, 310), Some((700, Range::new(1_000, 1_010))));
        assert_eq!(live_span(700, Some(400), selection, 700), None);
        assert_eq!(live_span(700, Some(400), selection, 730), Some((300, Range::new(1_000, 1_030))));
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
            placement_offset: 0,
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
            placement_offset: 0,
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

    #[test]
    fn compensated_origin_is_used_before_punch_crop_and_can_precede_session_zero() {
        let dir = std::env::temp_dir().join(format!("soundcraft-compensated-{}", std::process::id()));
        let path = dir.join("Take.wav");
        write_take(&path, 1_500);
        let mut e = Engine::default();
        let track = armed(&mut e, "Take", "mono", "In 1");
        let mut plan = Recording { start: 0, pass: None, punch: Some(Range::new(400, 1000)), placement_offset: 128, rate: 48_000, takes: vec![] };
        place_recording(&mut e, &plan, vec![(track, path.clone())]);
        let clip = &e.session().track(track).unwrap().clips()[0];
        assert_eq!((clip.start, clip.source_offset(), clip.length), (400, 528, 600));
        e.execute("edit.undo", &json!({})).unwrap();
        plan.punch = None;
        place_recording(&mut e, &plan, vec![(track, path)]);
        let clip = &e.session().track(track).unwrap().clips()[0];
        assert_eq!((clip.start, clip.source_offset(), clip.length), (0, 128, 1372));
        assert_eq!(live_span(-128, None, None, 1500), Some((-128, Range::new(0, 1372))));
        let _ = std::fs::remove_dir_all(dir);
    }
}
