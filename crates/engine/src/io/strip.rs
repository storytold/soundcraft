#[cfg(test)]
use super::import::add_source;
use super::{Arc, AtomicBool, Clip, EngineError, Ordering, Range, Result, Samples, Session, TrackId};

/// Limit a range used for rendering to the track's material (plus a second), so a huge
/// selection never renders hours of silence.
pub fn bounded_to_track(s: &Session, t: TrackId, r: Range) -> Range {
    let Some(tr) = s.track(t) else { return Range::point(r.start) };
    let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0).saturating_add(s.sample_rate.samples(1.0));
    let start = r.start.max(0).min(end);
    Range::new(start, r.end.min(end).max(start))
}

/// Transient positions (absolute) in a track's clips within `r`.
pub fn transients_in(s: &Session, t: TrackId, r: Range, sens: f32) -> Vec<Samples> {
    let r = if r.is_empty() { Range::new(0, s.content_end()) } else { r };
    let r = bounded_to_track(s, t, r);
    let audio = soundcraft_mix::render_clips(s, t, r);
    soundcraft_dsp::offline::detect_transients(&audio, s.sample_rate.as_f64() as f32, sens).into_iter().map(|i| r.start + i as i64).collect()
}

/// What Strip Silence does with the clips inside the selection once it has found the silence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripMode {
    /// Remove the silent parts and keep the parts that hold audio.
    Strip,
    /// Remove the parts that hold audio and keep the silent parts.
    Extract,
    /// Separate the clips at the boundaries of the silent parts without removing any clip.
    Separate,
}

impl StripMode {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "strip" => Some(Self::Strip),
            "extract" => Some(Self::Extract),
            "separate" => Some(Self::Separate),
            _ => None,
        }
    }
}

/// The names Strip Silence gives the clips it leaves in the selection: `name`, then `zeros` zeros,
/// then a number that starts at `number` and counts up one clip at a time, then `suffix`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StripNames {
    pub name: String,
    pub number: u64,
    pub zeros: usize,
    pub suffix: String,
}

impl Default for StripNames {
    fn default() -> Self {
        Self { name: String::new(), number: 1, zeros: 0, suffix: String::new() }
    }
}

impl StripNames {
    /// The largest first number.
    pub const MAX_NUMBER: u64 = soundcraft_model::AutoName::MAX_NUMBER;
    /// The most zeros before the number.
    pub const MAX_ZEROS: usize = soundcraft_model::AutoName::MAX_ZEROS;

    /// The name of the clip `i` places after the first one.
    pub fn clip_name(&self, i: u64) -> String {
        format!("{}{}{}{}", self.name, "0".repeat(self.zeros), self.number.saturating_add(i), self.suffix)
    }
}

impl From<&soundcraft_model::AutoName> for StripNames {
    fn from(a: &soundcraft_model::AutoName) -> Self {
        Self { name: a.name.clone(), number: a.number, zeros: a.zeros, suffix: a.suffix.clone() }
    }
}

/// Names the audio clips that lie inside `r` on `t` with `names`, in time order, the first of them
/// `next` places after `names.number`. Returns how many clips it named.
pub fn name_stripped_clips(s: &mut Session, t: TrackId, r: Range, names: &StripNames, next: u64) -> u64 {
    let r = bounded_to_track(s, t, r);
    let Some(pl) = s.track_mut(t).and_then(|x| x.playlist_mut()) else { return 0 };
    let mut inside: Vec<&mut Clip> = pl.clips.iter_mut().filter(|c| c.is_audio() && c.start >= r.start && c.end() <= r.end).collect();
    inside.sort_by_key(|c| (c.start, c.id.0));
    let mut named = 0;
    for c in inside {
        c.name = names.clip_name(next.saturating_add(named));
        named += 1;
    }
    named
}

/// Strip Silence: separate the clips inside `r` where silence starts and ends, then keep only the
/// non-silent ranges (`Strip`), only the silent ranges (`Extract`), or every piece (`Separate`).
/// `pad` is the clip start and end pad added around each non-silent range. Returns the number of
/// ranges kept: non-silent ranges for `Strip` and `Separate`, silent ranges that still hold a clip for
/// `Extract`. The new pieces that are kept take Auto Rename names in time order, and the removed
/// ones do not. When `named` is true the caller names the audio clips left inside `r` with
/// [`name_stripped_clips`], so those clips do not take Auto Rename names either.
#[allow(clippy::too_many_arguments)]
pub fn strip_silence(
    s: &mut Session,
    t: TrackId,
    r: Range,
    thr: f32,
    min_len: Samples,
    pad: (Samples, Samples),
    mode: StripMode,
    named: bool,
) -> Result<usize> {
    let r = bounded_to_track(s, t, r);
    let keep = non_silent(s, t, r, thr, min_len, pad, &AtomicBool::new(false))?;
    let mut cut_points = vec![r.start];
    for (a, b) in &keep {
        cut_points.push(*a);
        cut_points.push(*b);
    }
    cut_points.push(r.end);
    let old: std::collections::HashSet<_> = s.track(t).map(|x| x.clips().iter().map(|c| c.id).collect()).unwrap_or_default();
    for p in &cut_points {
        crate::edit::separate_at_if(s, t, *p, |_| false);
    }
    let kept = strip_pieces(s, t, r, &keep, mode);
    crate::edit::auto_name_new_clips(s, t, &old, |c| !(named && c.is_audio() && c.start >= r.start && c.end() <= r.end));
    Ok(kept)
}

/// Removes the pieces that Strip Silence does not keep in `mode` and returns the number of ranges kept.
fn strip_pieces(s: &mut Session, t: TrackId, r: Range, keep: &[(Samples, Samples)], mode: StripMode) -> usize {
    if mode == StripMode::Separate {
        return keep.len();
    }
    let mut gaps = Vec::new();
    let mut prev = r.start;
    for (a, b) in keep {
        if *a > prev {
            gaps.push(Range::new(prev, *a));
        }
        prev = *b;
    }
    if prev < r.end {
        gaps.push(Range::new(prev, r.end));
    }
    let audio: Vec<Range> = keep.iter().map(|(a, b)| Range::new(*a, *b)).collect();
    let remove = if mode == StripMode::Extract { &audio } else { &gaps };
    let inside = |c: &Clip, g: &Range| c.start >= g.start && c.end() <= g.end;
    let Some(pl) = s.track_mut(t).and_then(|x| x.playlist_mut()) else { return if mode == StripMode::Extract { 0 } else { keep.len() } };
    pl.clips.retain(|c| !remove.iter().any(|g| inside(c, g)));
    if mode == StripMode::Extract {
        return gaps.iter().filter(|g| pl.clips.iter().any(|c| inside(c, g))).count();
    }
    keep.len()
}

/// The ranges [`strip_silence`] finds on `t` inside `r` with the same settings: the non-silent ranges
/// with `pad` added, which Strip keeps. The session is read and not changed. Setting `stop` ends the
/// scan with an error before the next window of audio is rendered.
pub fn strip_silence_ranges(
    s: &Session,
    t: TrackId,
    r: Range,
    thr: f32,
    min_len: Samples,
    pad: (Samples, Samples),
    stop: &AtomicBool,
) -> Result<Vec<Range>> {
    let r = bounded_to_track(s, t, r);
    Ok(non_silent(s, t, r, thr, min_len, pad, stop)?.into_iter().map(|(a, b)| Range::new(a, b)).collect())
}

/// A Strip Silence scan of a session's tracks, which can run on another thread
/// (`cmd::strip_silence_scan` makes one from Strip Silence's params).
#[derive(Clone)]
pub struct StripScan {
    pub session: Arc<Session>,
    pub tracks: Vec<TrackId>,
    pub range: Range,
    pub threshold_db: f32,
    pub min_len: Samples,
    pub pad: (Samples, Samples),
    /// Set from another thread to end [`StripScan::run`] early, with an error.
    pub stop: Arc<AtomicBool>,
}

impl StripScan {
    /// The ranges Strip keeps on each track ([`strip_silence_ranges`]).
    pub fn run(&self) -> Result<Vec<(TrackId, Vec<Range>)>> {
        let s = &self.session;
        self.tracks
            .iter()
            .map(|t| Ok((*t, strip_silence_ranges(s, *t, self.range, self.threshold_db, self.min_len, self.pad, &self.stop)?)))
            .collect()
    }
}

/// Most samples (over all channels) Strip Silence renders at once.
const STRIP_WINDOW_SAMPLES: usize = 1 << 24;

/// The non-silent ranges of `t`'s clips in `r`. The audio is rendered a window at a time where
/// audio clips play, so all of `r` is scanned however long it is (one render stops at
/// `MAX_RENDER_SAMPLES`); at most a day of clips, Pro Tools' longest session. `pad` is the clip start
/// and end pad. Setting `stop` ends the scan with an error before the next window is rendered.
fn non_silent(
    s: &Session,
    t: TrackId,
    r: Range,
    thr: f32,
    min_len: Samples,
    (pre, post): (Samples, Samples),
    stop: &AtomicBool,
) -> Result<Vec<(Samples, Samples)>> {
    let Some(tr) = s.track(t) else { return Ok(Vec::new()) };
    let mut clips: Vec<Range> = tr.clips().iter().filter(|c| c.is_audio() && !c.muted).filter_map(|c| c.range().intersect(&r)).collect();
    clips.sort_by_key(|x| x.start);
    let mut spans: Vec<Range> = Vec::new();
    for x in clips {
        match spans.last_mut() {
            Some(last) if x.start <= last.end => last.end = last.end.max(x.end),
            _ => spans.push(x),
        }
    }
    if spans.iter().map(Range::len).fold(0, i64::saturating_add) > s.sample_rate.samples(86_400.0) {
        return Err(EngineError::BadParams("edit.strip_silence".into(), format!("`{}` has more than a day of clips to scan", tr.name)));
    }
    let window = i64::try_from(STRIP_WINDOW_SAMPLES / tr.channels().max(1)).unwrap_or(i64::MAX);
    let len = |n: Samples| u64::try_from(n).unwrap_or(0);
    let mut scan = soundcraft_dsp::offline::SilenceScan::new(thr, len(min_len));
    let mut at = r.start;
    for x in spans {
        scan.skip(len(x.start - at));
        at = x.start;
        while at < x.end {
            if stop.load(Ordering::Relaxed) {
                return Err(EngineError::Disabled("edit.strip_silence_preview".into(), "the scan was stopped".into()));
            }
            let end = at.saturating_add(window).min(x.end);
            scan.push(&soundcraft_mix::render_clips(s, t, Range::new(at, end)));
            at = end;
        }
    }
    scan.skip(len(r.end - at));
    Ok(scan.finish(len(pre), len(post)).into_iter().map(|(a, b)| (r.start.saturating_add_unsigned(a), r.start.saturating_add_unsigned(b))).collect())
}

#[cfg(test)]
mod strip_tests {
    use super::super::{AudioBuffer, Engine, FileFormat, Value};
    use super::*;
    use serde_json::json;

    /// A stereo track with `length`-long clips at `starts`, each playing a second of steady tone.
    fn tones(starts: &[Samples], length: Samples) -> (Engine, TrackId) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id;
        let s = e.session_mut();
        let tone = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]; 2] };
        let src = add_source(s, "tone", tone, None, FileFormat::Wav);
        for at in starts {
            let id = s.new_clip_id();
            crate::edit::place_clip(s, t, Clip::audio(id, "tone", src, 0, *at, length));
        }
        (e, t)
    }

    #[test]
    fn clips_past_what_one_render_holds_are_kept() {
        // 100 minutes in: a stereo render stops at 2^28 frames, about 93 minutes.
        let far = 48_000 * 6000;
        let (mut e, t) = tones(&[0, far], 48_000);
        let r = e.execute("edit.strip_silence", &json!({"track": t.0, "start": 0, "end": far + 96_000})).unwrap();
        assert_eq!(r["clips"], 2);
        assert!(e.session().track(t).unwrap().clips().iter().any(|c| c.start >= far));
    }

    /// A stereo track with one three-second clip: a second of tone, a second of silence, a second of tone.
    fn tone_gap_tone() -> (Engine, TrackId) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id;
        let s = e.session_mut();
        let mut ch = vec![0.5; 144_000];
        ch[48_000..96_000].fill(0.0);
        let src = add_source(s, "tone gap tone", AudioBuffer { sample_rate: 48_000, channels: vec![ch.clone(), ch] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(id, "tone gap tone", src, 0, 0, 144_000));
        (e, t)
    }

    fn strip_mode(mode: &str) -> (Value, Vec<(Samples, Samples)>) {
        let (mut e, t) = tone_gap_tone();
        let p = json!({"track": t.0, "start": 0, "end": 144_000, "pad_before_ms": 0, "pad_after_ms": 0, "mode": mode});
        let r = e.execute("edit.strip_silence", &p).unwrap();
        (r, e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end())).collect())
    }

    #[test]
    fn strip_keeps_the_audio_and_removes_the_silence() {
        let (r, clips) = strip_mode("strip");
        assert_eq!(r["clips"], 2);
        assert_eq!(clips, vec![(0, 48_000), (96_000, 144_000)]);
    }

    #[test]
    fn extract_keeps_the_silence_and_removes_the_audio() {
        let (r, clips) = strip_mode("extract");
        assert_eq!(r["clips"], 1);
        assert_eq!(clips, vec![(48_000, 96_000)]);
    }

    #[test]
    fn separate_cuts_at_the_silence_and_removes_nothing() {
        let (r, clips) = strip_mode("separate");
        assert_eq!(r["clips"], 2);
        assert_eq!(clips, vec![(0, 48_000), (48_000, 96_000), (96_000, 144_000)]);
    }

    #[test]
    fn the_rectangles_are_what_strip_keeps_and_change_nothing() {
        let (mut e, t) = tone_gap_tone();
        let p = json!({"track": t.0, "start": 0, "end": 144_000, "pad_before_ms": 10, "pad_after_ms": 20});
        let rev = e.revision;
        let r = e.execute("edit.strip_silence_preview", &p).unwrap();
        assert_eq!(e.revision, rev, "the preview changed the session");
        assert_eq!(e.session().track(t).unwrap().clips().len(), 1);
        assert_eq!(r["tracks"][0]["track"], t.0);
        let ranges: Vec<(Samples, Samples)> =
            r["tracks"][0]["ranges"].as_array().unwrap().iter().map(|x| (x[0].as_i64().unwrap(), x[1].as_i64().unwrap())).collect();
        assert_eq!(ranges, vec![(0, 48_960), (95_520, 144_000)]);
        e.execute("edit.strip_silence", &p).unwrap();
        let kept: Vec<(Samples, Samples)> = e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end())).collect();
        assert_eq!(kept, ranges);
    }

    #[test]
    fn a_stopped_scan_ends_with_an_error() {
        let (e, t) = tone_gap_tone();
        let scan = crate::cmd::strip_silence_scan(&e, &json!({"track": t.0, "start": 0, "end": 144_000})).unwrap();
        assert_eq!(scan.run().unwrap()[0].1.len(), 2);
        scan.stop.store(true, Ordering::Relaxed);
        assert!(scan.run().is_err());
    }

    #[test]
    fn the_rectangles_follow_the_threshold() {
        let (mut e, t) = tone_gap_tone();
        // The tone plays at -6 dB.
        let r = e.execute("edit.strip_silence_preview", &json!({"track": t.0, "start": 0, "end": 144_000, "threshold_db": -3})).unwrap();
        assert_eq!(r["tracks"][0]["ranges"], json!([]));
        let r = e.execute("edit.strip_silence_preview", &json!({"track": t.0, "start": 0, "end": 144_000, "threshold_db": -12})).unwrap();
        assert_eq!(r["tracks"][0]["ranges"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn an_unknown_strip_mode_is_refused() {
        let (mut e, t) = tone_gap_tone();
        for mode in [json!("remove"), json!(null), json!(true), json!(1)] {
            assert!(e.execute("edit.strip_silence", &json!({"track": t.0, "start": 0, "end": 144_000, "mode": mode})).is_err());
            assert_eq!(e.session().track(t).unwrap().clips().len(), 1);
        }
    }

    fn names(e: &Engine, t: TrackId) -> Vec<String> {
        e.session().track(t).unwrap().clips().iter().map(|c| c.name.clone()).collect()
    }

    #[test]
    fn strip_names_follow_the_guide_example() {
        let n = StripNames { name: "SFX".into(), number: 23, zeros: 1, suffix: ".Reel1".into() };
        assert_eq!((0..4).map(|i| n.clip_name(i)).collect::<Vec<_>>(), ["SFX023.Reel1", "SFX024.Reel1", "SFX025.Reel1", "SFX026.Reel1"]);
    }

    #[test]
    fn strip_names_the_clips_it_keeps() {
        let (mut e, t) = tone_gap_tone();
        let p = json!({"track": t.0, "start": 0, "end": 144_000, "pad_before_ms": 0, "pad_after_ms": 0, "name": "SFX", "number": 23, "zeros": 1, "suffix": ".Reel1"});
        let r = e.execute("edit.strip_silence", &p).unwrap();
        assert_eq!(r["named"], 2);
        assert_eq!(names(&e, t), ["SFX023.Reel1", "SFX024.Reel1"]);
    }

    #[test]
    fn separate_names_every_piece_in_time_order() {
        let (mut e, t) = tone_gap_tone();
        let p = json!({"track": t.0, "start": 0, "end": 144_000, "pad_before_ms": 0, "pad_after_ms": 0, "mode": "separate", "name": "Take "});
        e.execute("edit.strip_silence", &p).unwrap();
        assert_eq!(names(&e, t), ["Take 1", "Take 2", "Take 3"]);
    }

    #[test]
    fn strip_numbers_run_on_from_one_track_to_the_next() {
        let (mut e, a) = tone_gap_tone();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let b = e.session().tracks[1].id;
        let s = e.session_mut();
        let src = s.sources[0].id;
        let id = s.new_clip_id();
        crate::edit::place_clip(s, b, Clip::audio(id, "copy", src, 0, 0, 144_000));
        let p = json!({"tracks": [a.0, b.0], "start": 0, "end": 144_000, "pad_before_ms": 0, "pad_after_ms": 0, "name": "Hit", "zeros": 2});
        let r = e.execute("edit.strip_silence", &p).unwrap();
        assert_eq!(r["named"], 4);
        assert_eq!(names(&e, a), ["Hit001", "Hit002"]);
        assert_eq!(names(&e, b), ["Hit003", "Hit004"]);
    }

    #[test]
    fn strip_without_a_name_keeps_the_clip_names() {
        let (mut e, t) = tone_gap_tone();
        let r = e.execute("edit.strip_silence", &json!({"track": t.0, "start": 0, "end": 144_000, "name": " "})).unwrap();
        assert_eq!(r["named"], 0);
        assert_eq!(names(&e, t), ["tone gap tone", "tone gap tone"]);
    }

    #[test]
    fn strip_names_leave_the_auto_rename_sequence_unbroken() {
        let (mut e, t) = tone_gap_tone();
        let id = e.session().track(t).unwrap().clips()[0].id;
        e.execute("clip.auto_rename", &json!({"clips": [id], "name": "Take"})).unwrap();
        let p = json!({"track": t.0, "start": 0, "end": 120_000, "pad_before_ms": 0, "pad_after_ms": 0, "mode": "separate", "name": "SFX"});
        assert_eq!(e.execute("edit.strip_silence", &p).unwrap()["named"], 3);
        // Strip Silence does not name the piece after the range, so that piece takes the first Auto Rename name.
        assert_eq!(names(&e, t), ["SFX1", "SFX2", "SFX3", "Take1"]);
        e.execute("edit.separate", &json!({"tracks": [t], "at": 132_000})).unwrap();
        assert_eq!(names(&e, t)[4], "Take2");
    }

    #[test]
    fn only_the_pieces_strip_silence_keeps_take_auto_rename_numbers() {
        for (mode, kept) in [("strip", ["tone gap tone", "Take1", "Take2"].as_slice()), ("extract", ["Take1", "Take2"].as_slice())] {
            let (mut e, t) = tone_gap_tone();
            let id = e.session().track(t).unwrap().clips()[0].id;
            e.execute("clip.auto_rename", &json!({"clips": [id], "name": "Take"})).unwrap();
            let p = json!({"track": t.0, "start": 0, "end": 120_000, "pad_before_ms": 0, "pad_after_ms": 0, "mode": mode});
            e.execute("edit.strip_silence", &p).unwrap();
            assert_eq!(names(&e, t), kept, "{mode}");
        }
    }

    #[test]
    fn bad_naming_params_are_refused() {
        for bad in [
            json!({"suffix": ".Reel1"}),
            json!({"name": "", "number": 2}),
            json!({"name": 7}),
            json!({"name": "SFX", "number": -1}),
            json!({"name": "SFX", "number": StripNames::MAX_NUMBER + 1}),
            json!({"name": "SFX", "zeros": StripNames::MAX_ZEROS + 1}),
            json!({"name": "SFX", "suffix": null}),
            json!({"name": "S".repeat(300)}),
        ] {
            let (mut e, t) = tone_gap_tone();
            let mut p = json!({"track": t.0, "start": 0, "end": 144_000});
            p.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
            assert!(e.execute("edit.strip_silence", &p).is_err(), "{bad} was taken");
            assert_eq!(names(&e, t), ["tone gap tone"], "{bad} changed the track");
        }
    }

    #[test]
    fn more_than_a_day_of_clips_is_refused() {
        let (mut e, t) = tones(&[0], 48_000 * 86_401);
        assert!(e.execute("edit.strip_silence", &json!({"track": t.0, "start": 0, "end": 48_000i64 * 86_402})).is_err());
        assert_eq!(e.session().track(t).unwrap().clips().len(), 1);
    }
}
