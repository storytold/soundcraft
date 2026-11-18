//! Noise reduction: learn the background noise from a passage of "silence", then remove it into a
//! copy of the track (or of the selected part of it), leaving the original as it was.
//!
//! The learned profile lives on the [`Engine`], not in the document: it outlasts undo and opening
//! another session, so the noise of one room can clean every take recorded in it.
use super::{CommandSpec, always, bad, clip_ids_param, f32_or, has_selection, range_param, tracks_param};
use crate::{Engine, Result, cmd};
use craft_audio::AudioBuffer;
use serde_json::{Value, json};
use soundcraft_dsp::denoise::{self, NoiseProfile, ReduceSettings};
use soundcraft_model::{Clip, ClipContent, ClipId, FileFormat, TrackId, TrackKind};
use soundcraft_time::Range;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "audiosuite.noise_learn",
            "Learn Noise Profile",
            [],
            None,
            "{track?: id|name (default: the first selected audio track), start?, end? (default: the edit selection, or the selected clip)} — analyses a passage holding only background noise and keeps it as the noise profile (at least 43 ms; the first 5 minutes or so of a longer passage)",
            has_selection,
            learn
        ),
        cmd!(
            "audiosuite.noise_reduce",
            "Noise Reduction",
            [],
            None,
            "{reduction_db?: 0..48 (12), sensitivity_db?: 0..24 (6), smoothing?: 0..1 (0.5), mute_original?: true, clips?, tracks?, start?, end?} — removes the learned noise profile into a copy of each track, placed just after it; the original track is not changed except for being muted (mute_original). With a time range on tracks the copy holds just that part; otherwise the target clips, whole",
            can_reduce,
            reduce
        ),
        cmd!(
            query "audiosuite.noise_profile",
            "Noise Profile",
            [],
            None,
            "{data?: bool} — the learned noise profile: sample rate, channels, seconds, and its spectrum in dB per bin; with data, the whole profile, which audiosuite.noise_profile_set takes back",
            always,
            profile
        ),
        cmd!(
            noundo "audiosuite.noise_profile_set",
            "Set Noise Profile",
            [],
            None,
            "{profile: <a profile from audiosuite.noise_profile with data> | null} — replaces the noise profile; null forgets it",
            always,
            set_profile
        ),
    ]
}

fn can_reduce(e: &Engine) -> std::result::Result<(), String> {
    if e.noise_profile.is_none() {
        return Err("learn a noise profile first".into());
    }
    has_selection(e)
}

fn info(p: &NoiseProfile) -> Value {
    json!({"sample_rate": p.sample_rate, "channels": p.channels(), "frames": p.frames, "seconds": p.seconds(), "fft_size": p.fft_size})
}

/// The audio track to learn from: the one named, else the first selected audio track, else the
/// track of the first selected clip.
fn learn_track(e: &Engine, p: &Value) -> Result<Option<TrackId>> {
    let s = e.session();
    let audio = |t: &TrackId| s.track(*t).is_some_and(|t| t.kind == TrackKind::Audio);
    if p.get("track").is_some() {
        let one = json!({"tracks": [p.get("track").cloned().unwrap_or(Value::Null)]});
        return Ok(tracks_param(e, "audiosuite.noise_learn", &one)?.into_iter().find(audio));
    }
    if p.get("tracks").is_some() {
        return Ok(tracks_param(e, "audiosuite.noise_learn", p)?.into_iter().find(audio));
    }
    if let Some(t) = s.edit.selected_tracks.iter().find(|t| audio(t)) {
        return Ok(Some(*t));
    }
    Ok(clip_ids_param(e, p).into_iter().find_map(|id| s.find_clip(id).map(|(t, _)| t)).filter(audio))
}

/// The clip as `learn` and `reduce` hear it: without fades, clip gain and clip mute, which stay on
/// the clip for playback.
fn unprocessed(c: &mut Clip) {
    c.fade_in = Default::default();
    c.fade_out = Default::default();
    c.gain_db = 0.0;
    c.gain_env.clear();
    c.muted = false;
}

/// Whether every sample of `range` on `track` lies inside an audio clip.
fn covered_by_audio(s: &soundcraft_model::Session, track: TrackId, range: Range) -> bool {
    let Some(t) = s.track(track) else { return false };
    let mut spans: Vec<Range> =
        t.clips().iter().filter(|c| c.is_audio()).filter_map(|c| c.range().intersect(&range)).filter(|r| !r.is_empty()).collect();
    spans.sort_by_key(|r| r.start);
    let mut at = range.start;
    for r in spans {
        if r.start > at {
            return false;
        }
        at = at.max(r.end);
    }
    at >= range.end
}

fn learn(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "audiosuite.noise_learn";
    let track = learn_track(e, p)?.ok_or_else(|| bad(CMD, "select a passage of noise on an audio track"))?;
    let mut range = range_param(e, CMD, p)?;
    if range.is_empty() {
        // A clip selected on its own: learn the whole clip.
        let clips = clip_ids_param(e, p);
        if let Some(c) = clips.iter().find_map(|id| e.session().find_clip(*id).filter(|(t, _)| *t == track).map(|(_, c)| c.range())) {
            range = c;
        }
    }
    let s = e.session();
    let frames = usize::try_from(range.len()).unwrap_or(0).min(denoise::MAX_LEARN_SAMPLES);
    let range = Range::new(range.start, range.start.saturating_add(i64::try_from(frames).unwrap_or(0)));
    let min_ms = (denoise::MIN_LEARN_SAMPLES as f64 * 1000.0 / s.sample_rate.as_f64()).ceil();
    if frames < denoise::MIN_LEARN_SAMPLES {
        return Err(bad(CMD, format!("select at least {min_ms:.0} ms of noise to learn")));
    }
    // A gap between clips renders as digital silence, which would teach "no noise" and replace a
    // useful profile: the whole passage must be audio from clips.
    if !covered_by_audio(s, track, range) {
        return Err(bad(CMD, "select noise inside the audio clips: the selection runs over a gap"));
    }
    // Without clip gain, fades and clip mute, the way `reduce` renders the audio it cleans: a clip
    // turned down 6 dB must not teach a noise 6 dB quieter than the one in its file.
    let mut raw = s.clone();
    for c in raw.track_mut(track).and_then(|t| t.playlist_mut()).map(|pl| pl.clips.iter_mut()).into_iter().flatten() {
        unprocessed(c);
    }
    let audio = soundcraft_mix::render_clips(&raw, track, range);
    // On failure the previous profile stays.
    let profile =
        NoiseProfile::learn(&audio, s.sample_rate.hz()).ok_or_else(|| bad(CMD, "this passage could not be analysed (its level is out of range)"))?;
    let name = s.track(track).map(|t| t.name.clone()).unwrap_or_default();
    let out = info(&profile);
    e.noise_profile = Some(profile);
    e.message(format!("Learned the noise profile from {name}"));
    Ok(out)
}

fn profile(e: &mut Engine, p: &Value) -> Result<Value> {
    let Some(prof) = &e.noise_profile else { return Ok(Value::Null) };
    let mut out = info(prof);
    if let Some(o) = out.as_object_mut() {
        o.insert("spectrum_db".into(), json!(prof.spectrum_db()));
        if p.get("data").and_then(Value::as_bool).unwrap_or(false) {
            o.insert("data".into(), serde_json::to_value(prof).unwrap_or(Value::Null));
        }
    }
    Ok(out)
}

fn set_profile(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "audiosuite.noise_profile_set";
    match p.get("profile") {
        None => Err(bad(CMD, "`profile` required (null forgets the profile)")),
        Some(Value::Null) => {
            e.noise_profile = None;
            Ok(Value::Null)
        }
        Some(v) => {
            let prof: NoiseProfile = serde_json::from_value(v.clone()).map_err(|err| bad(CMD, format!("not a noise profile: {err}")))?;
            if !prof.is_valid() {
                return Err(bad(CMD, "not a usable noise profile"));
            }
            let out = info(&prof);
            e.noise_profile = Some(prof);
            Ok(out)
        }
    }
}

/// The parts to process: (track, clip, part of the clip). A time range on tracks processes just
/// the parts of their clips inside it; otherwise each target clip is processed whole.
fn segments(e: &Engine, p: &Value) -> Result<Vec<(TrackId, ClipId, Range)>> {
    const CMD: &str = "audiosuite.noise_reduce";
    let s = e.session();
    let names_clips = p.get("clips").is_some() || p.get("clip").is_some();
    let range = range_param(e, CMD, p)?;
    let tracks = if p.get("tracks").is_some() { tracks_param(e, CMD, p)? } else { s.edit.selected_tracks.clone() };
    if !names_clips && !range.is_empty() && !tracks.is_empty() {
        let mut out = Vec::new();
        for t in tracks.iter().filter_map(|t| s.track(*t)).filter(|t| t.kind == TrackKind::Audio) {
            for c in t.clips().iter().filter(|c| c.is_audio()) {
                if let Some(part) = c.range().intersect(&range).filter(|r| !r.is_empty()) {
                    out.push((t.id, c.id, part));
                }
            }
        }
        return Ok(out);
    }
    // Whole clips. Named tracks still limit which: a selected clip on another track is left alone.
    let named = p.get("tracks").is_some();
    Ok(clip_ids_param(e, p)
        .into_iter()
        .filter_map(|id| s.find_clip(id).filter(|(t, c)| c.is_audio() && (!named || tracks.contains(t))).map(|(t, c)| (t, id, c.range())))
        .collect())
}

fn reduce(e: &mut Engine, p: &Value) -> Result<Value> {
    reduce_in(e, p, None)
}

/// `audiosuite.noise_reduce`, cleaning only what falls inside `window` when one is given.
fn reduce_in(e: &mut Engine, p: &Value, window: Option<Range>) -> Result<Value> {
    const CMD: &str = "audiosuite.noise_reduce";
    let profile = e.noise_profile.clone().ok_or_else(|| bad(CMD, "learn a noise profile first (audiosuite.noise_learn)"))?;
    let sr = e.session().sample_rate;
    if profile.sample_rate != sr.hz() {
        return Err(bad(CMD, format!("the noise profile was learned at {} Hz, but this session runs at {} Hz", profile.sample_rate, sr.hz())));
    }
    let settings = ReduceSettings {
        reduction_db: f32_or(p, "reduction_db", ReduceSettings::default().reduction_db),
        sensitivity_db: f32_or(p, "sensitivity_db", ReduceSettings::default().sensitivity_db),
        smoothing: f32_or(p, "smoothing", ReduceSettings::default().smoothing),
    }
    .sanitized();
    let mute_original = p.get("mute_original").and_then(Value::as_bool).unwrap_or(true);
    let mut parts = segments(e, p)?;
    if let Some(w) = window {
        parts = parts.into_iter().filter_map(|(t, id, r)| r.intersect(&w).filter(|r| !r.is_empty()).map(|r| (t, id, r))).collect();
    }
    if parts.is_empty() {
        return Err(bad(CMD, "select audio clips, or a range on audio tracks, first"));
    }
    // Refuse before changing anything, rather than stop halfway; the cleaned audio is all kept
    // until the copies are made, so cap the total too.
    let mut budget = soundcraft_mix::MAX_RENDER_SAMPLES;
    for (track, id, part) in &parts {
        let channels = e.session().track(*track).map_or(1, |t| t.channels()).max(1);
        let (name, rendered) =
            e.session().find_clip(*id).map_or((String::new(), part.len()), |(_, c)| (c.name.clone(), with_handles(c, *part).len()));
        // The render includes the handles: a part that fits but whose handles do not would come
        // back cut short.
        crate::io::refuse_longer(e.session(), CMD, &name, "clean", rendered, crate::io::render_limit(channels))?;
        budget = budget
            .checked_sub(usize::try_from(part.len()).unwrap_or(usize::MAX).saturating_mul(channels))
            .ok_or_else(|| bad(CMD, "too much audio to clean in one command"))?;
    }
    // Clean every part first, from the untouched session.
    let mut cleaned: Vec<(TrackId, Clip, Vec<Vec<f32>>)> = Vec::new();
    for (track, id, part) in parts {
        let Some(clip) = e.session().find_clip(id).map(|(_, c)| c.clone()) else { continue };
        let audio = clean_part(e.session(), &profile, settings, track, &clip, part);
        // Every channel must cover the whole part, or the copy would be short; checked before
        // anything is changed.
        let want = usize::try_from(part.len()).unwrap_or(0);
        if audio.is_empty() || audio.iter().any(|c| c.len() != want) {
            return Err(bad(CMD, format!("could not render all of {} to clean it", clip.name)));
        }
        {
            // The clip as it will sit on the copy: cut to the part, its fades kept only at its own edges.
            let mut piece = clip.clone();
            if part.start > clip.start {
                piece.trim_start_to(part.start);
                piece.fade_in = Default::default();
            }
            if part.end < clip.end() {
                piece.trim_end_to(part.end);
                piece.fade_out = Default::default();
            }
            cleaned.push((track, piece, audio));
        }
    }
    if cleaned.is_empty() {
        return Err(bad(CMD, "nothing could be processed"));
    }
    // One copy of each source track, just after it, holding the cleaned audio. The original
    // track is left as it is (muted by default, so the copy is what plays).
    let s = e.session_mut();
    let mut copies: Vec<(TrackId, TrackId)> = Vec::new();
    let mut clips = Vec::new();
    for (track, piece, audio) in cleaned {
        let copy = match copies.iter().find(|(from, _)| *from == track) {
            Some((_, c)) => *c,
            None => {
                let Some(idx) = s.track_index(track) else { continue };
                let Some(mut nt) = s.tracks.get(idx).cloned() else { continue };
                nt.id = TrackId(s.alloc());
                nt.name = s.unique_track_name(&format!("{}-NR", nt.name));
                for pl in &mut nt.playlists {
                    pl.clips.clear();
                }
                // The copy plays as the original would: it keeps a solo (clearing it would leave
                // a soloed original muted and its copy silenced by the solo), but not a mute.
                nt.mixer.mute = false;
                nt.mixer.record_arm = false;
                let id = nt.id;
                s.tracks.insert((idx + 1).min(s.tracks.len()), nt);
                if mute_original && let Some(t) = s.track_mut(track) {
                    t.mixer.mute = true;
                }
                copies.push((track, id));
                id
            }
        };
        let frames = audio.first().map_or(0, Vec::len);
        let new_name = format!("{}-NR", piece.name);
        let src = crate::io::add_source(s, &new_name, AudioBuffer { sample_rate: sr.hz(), channels: audio }, None, FileFormat::Wav);
        let mut nc = piece;
        nc.id = s.new_clip_id();
        nc.name = new_name;
        nc.content = ClipContent::Audio { source: src, offset: 0 };
        nc.length = i64::try_from(frames).unwrap_or(nc.length);
        // The render already has the clip's stretch applied.
        nc.stretch = 1.0;
        nc.clamp_fades();
        clips.push(nc.id.0);
        // Straight onto the copy's playlist, keeping the source's arrangement: clips that overlap
        // in a crossfade must not clear each other, as placing one over the other would.
        if let Some(pl) = s.track_mut(copy).and_then(|t| t.playlist_mut()) {
            pl.clips.push(nc);
        }
    }
    for (_, copy) in &copies {
        if let Some(pl) = s.track_mut(*copy).and_then(|t| t.playlist_mut()) {
            pl.sort();
        }
    }
    let tracks: Vec<TrackId> = copies.iter().map(|(_, c)| *c).collect();
    s.edit.selected_tracks = tracks.clone();
    s.edit.selected_clips.clear();
    Ok(json!({"processed": clips.len(), "clips": clips, "tracks": tracks}))
}

/// `part` with up to one analysis frame of `clip` on each side: what [`clean_part`] renders.
fn with_handles(clip: &Clip, part: Range) -> Range {
    let handle = i64::try_from(denoise::FFT_SIZE).unwrap_or(0);
    Range::new(part.start.saturating_sub(handle).max(clip.start), part.end.saturating_add(handle).min(clip.end()))
}

/// `part` of `clip` on `track`, cleaned: rendered without the clip's fades, gain and mute (those stay
/// on the clip, editable), with up to one analysis frame of the clip's own audio either side so the
/// edges of the part are analysed with real context.
fn clean_part(
    s: &soundcraft_model::Session,
    profile: &NoiseProfile,
    settings: ReduceSettings,
    track: TrackId,
    clip: &Clip,
    part: Range,
) -> Vec<Vec<f32>> {
    let mut tmp = s.clone();
    if let Some(c) = tmp.find_clip_mut(clip.id) {
        unprocessed(c);
    }
    if let Some(pl) = tmp.track_mut(track).and_then(|t| t.playlist_mut()) {
        pl.clips.retain(|c| c.id == clip.id);
    }
    let around = with_handles(clip, part);
    let audio = soundcraft_mix::render_clips(&tmp, track, around);
    let out = denoise::reduce(&audio, profile, settings);
    let skip = usize::try_from(part.start.saturating_sub(around.start)).unwrap_or(0);
    let len = usize::try_from(part.len()).unwrap_or(0);
    out.iter().map(|c| c.get(skip..).unwrap_or_default().iter().take(len).copied().collect()).collect()
}

/// Longest stretch [`preview`] cleans.
pub const PREVIEW_SECONDS: f64 = 30.0;

/// The range [`preview`] cleans and loops for `p`: from the start of the first target, at most
/// [`PREVIEW_SECONDS`] long. Nothing is rendered.
pub fn preview_range(e: &Engine, p: &Value) -> Result<Range> {
    let parts = segments(e, p)?;
    let start = parts
        .iter()
        .map(|(_, _, r)| r.start)
        .min()
        .ok_or_else(|| bad("audiosuite.noise_reduce", "select audio clips, or a range on audio tracks, first"))?;
    let end = parts.iter().map(|(_, _, r)| r.end).max().unwrap_or(start);
    Ok(Range::new(start, end.min(start.saturating_add(e.session().sample_rate.samples(PREVIEW_SECONDS)))))
}

/// The session as `audiosuite.noise_reduce` with `p` would leave it, for listening before applying,
/// and the range to loop. The original tracks are muted and their cleaned copies sit beside them;
/// only the first [`PREVIEW_SECONDS`] of the targets (as `noise_reduce` resolves them: named clips,
/// or a range on tracks) are cleaned. The document is not changed.
pub fn preview(e: &Engine, p: &Value) -> Result<(std::sync::Arc<soundcraft_model::Session>, Range)> {
    let window = preview_range(e, p)?;
    let mut tmp = Engine::new(e.session().clone());
    tmp.noise_profile = e.noise_profile.clone();
    let mut p = p.clone();
    if let Some(o) = p.as_object_mut() {
        o.insert("mute_original".into(), json!(true));
    }
    reduce_in(&mut tmp, &p, Some(window))?;
    Ok((tmp.session_arc(), window))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Session};

    /// Deterministic white noise in -amp..amp.
    fn hiss(len: usize, amp: f32) -> Vec<f32> {
        let mut s = 12_345u32;
        (0..len)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// One mono track with one 2-second clip at 0: hiss, plus a 1 kHz tone in its second half.
    fn take() -> (Engine, TrackId, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Sax"));
        let audio: Vec<f32> = hiss(96_000, 0.05)
            .into_iter()
            .enumerate()
            .map(|(i, n)| n + if i >= 48_000 { 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin() } else { 0.0 })
            .collect();
        let src = crate::io::add_source(&mut s, "take", AudioBuffer { sample_rate: 48_000, channels: vec![audio] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, "take", src, 0, 0, 96_000);
        c.gain_db = -3.0;
        crate::edit::place_clip(&mut s, t, c);
        (Engine::new(s), t, id)
    }

    fn track_audio(e: &Engine, t: TrackId, r: Range) -> Vec<f32> {
        // Without clip gain, to compare with the source.
        let mut s = e.session().clone();
        for c in s.track_mut(t).and_then(|t| t.playlist_mut()).map(|p| p.clips.iter_mut()).into_iter().flatten() {
            c.gain_db = 0.0;
        }
        soundcraft_mix::render_clips(&s, t, r).into_iter().next().unwrap_or_default()
    }

    #[test]
    fn reduce_needs_a_learned_profile() {
        let (mut e, _, id) = take();
        let err = e.execute("audiosuite.noise_reduce", &json!({"clips": [id.0]})).unwrap_err().to_string();
        assert!(err.contains("learn a noise profile"), "{err}");
        assert_eq!(e.execute("audiosuite.noise_profile", &json!({})).unwrap(), Value::Null);
    }

    #[test]
    fn learn_refuses_a_passage_too_short() {
        let (mut e, t, _) = take();
        let err = e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 100})).unwrap_err().to_string();
        assert!(err.contains("at least 43 ms"), "{err}");
        assert!(e.noise_profile.is_none());
    }

    #[test]
    fn the_selected_part_is_cleaned_into_a_copy_of_the_track() {
        let (mut e, t, id) = take();
        let before = track_audio(&e, t, Range::new(0, 96_000));
        let learned = e.execute("audiosuite.noise_learn", &json!({"track": "Sax", "start": 0, "end": 24_000})).unwrap();
        assert_eq!((learned["channels"].as_u64(), learned["sample_rate"].as_u64()), (Some(1), Some(48_000)));
        // Learning changes nothing in the session and leaves nothing to undo.
        assert!(!e.can_undo());

        // Clean 0.25 s .. 1.75 s.
        let out = e.execute("audiosuite.noise_reduce", &json!({"tracks": [t.0], "start": 12_000, "end": 84_000, "reduction_db": 24})).unwrap();
        assert_eq!(out["processed"], 1);
        let s = e.session();
        assert_eq!(s.tracks.len(), 2);
        // The original keeps its one clip, untouched, and is muted.
        let orig = &s.tracks[0];
        assert_eq!((orig.id, orig.clips().len(), orig.clips()[0].id, orig.clips()[0].length), (t, 1, id, 96_000));
        assert!(orig.mixer.mute);
        assert_eq!(track_audio(&e, t, Range::new(0, 96_000)), before);
        // The copy, just after it, holds only the cleaned part, with the clip's gain.
        let copy = &s.tracks[1];
        assert_eq!(copy.name, "Sax-NR");
        assert!(!copy.mixer.mute);
        let c = &copy.clips()[0];
        assert_eq!((copy.clips().len(), c.start, c.length, c.name.as_str()), (1, 12_000, 72_000, "take-NR"));
        assert!((c.gain_db + 3.0).abs() < 1e-6);
        assert_eq!(s.edit.selected_tracks, vec![copy.id]);

        let after = track_audio(&e, copy.id, Range::new(0, 96_000));
        assert!(after[..12_000].iter().chain(&after[84_000..]).all(|v| *v == 0.0), "the copy is silent outside the part");
        let drop = soundcraft_dsp::gain_to_db(rms(&after[16_000..44_000]) / rms(&before[16_000..44_000]));
        assert!(drop < -18.0, "hiss dropped only {drop} dB");
        let tone = soundcraft_dsp::gain_to_db(rms(&after[52_000..80_000]) / rms(&before[52_000..80_000]));
        assert!(tone.abs() < 1.0, "tone changed {tone} dB");

        // One undo removes the copy and unmutes the original.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().tracks.len(), 1);
        assert!(!e.session().tracks[0].mixer.mute);
        assert!(e.noise_profile.is_some(), "the profile outlives undo");
    }

    #[test]
    fn selected_clips_are_copied_whole_and_the_original_can_stay_unmuted() {
        let (mut e, t, id) = take();
        e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        e.session_mut().edit.selected_clips = vec![id];
        e.execute("audiosuite.noise_reduce", &json!({"mute_original": false})).unwrap();
        let s = e.session();
        assert!(!s.tracks[0].mixer.mute);
        assert_eq!(s.tracks[0].clips()[0].name, "take");
        let clips = s.tracks[1].clips();
        assert_eq!((clips.len(), clips[0].start, clips[0].length, clips[0].name.as_str()), (1, 0, 96_000, "take-NR"));
    }

    #[test]
    fn preview_leaves_the_document_alone_and_stops_at_its_limit() {
        let (mut e, t, _) = take();
        e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        let rev = e.revision;
        let undo = e.can_undo();
        let (s, r) = preview(&e, &json!({"tracks": [t.0], "start": 0, "end": 96_000, "mute_original": false})).unwrap();
        assert_eq!((r.start, r.end), (0, 96_000));
        assert_eq!((e.revision, e.can_undo(), e.session().tracks.len()), (rev, undo, 1));
        assert_eq!(s.tracks.len(), 2);
        assert!(s.tracks[0].mixer.mute, "the preview always plays the cleaned copy alone");
        assert_eq!(s.tracks[1].clips()[0].length, 96_000);

        let most = e.session().sample_rate.samples(PREVIEW_SECONDS);
        let mut long = e.session().clone();
        // A 60-second clip: the 2-second take played 30 times slower.
        let c = long.find_clip_mut(long.tracks[0].clips()[0].id).unwrap();
        c.stretch = 30.0;
        c.length = 96_000 * 30;
        let mut e2 = Engine::new(long);
        e2.noise_profile = e.noise_profile.clone();
        let (s, r) = preview(&e2, &json!({"tracks": [t.0], "start": 0, "end": 96_000 * 30})).unwrap();
        assert_eq!((s.tracks[1].clips()[0].length, r.len()), (most, most));
        // Named clips with no range are cut to the limit too, after they are resolved.
        let id = e2.session().tracks[0].clips()[0].id;
        let (s, r) = preview(&e2, &json!({"clips": [id.0]})).unwrap();
        assert_eq!((s.tracks[1].clips()[0].length, r.len()), (most, most));
    }

    #[test]
    fn the_profile_round_trips_and_a_wrong_rate_is_refused() {
        let (mut e, t, id) = take();
        e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        let full = e.execute("audiosuite.noise_profile", &json!({"data": true})).unwrap();
        assert_eq!(full["spectrum_db"].as_array().map(Vec::len), Some(denoise::BINS));
        let data = full["data"].clone();

        e.execute("audiosuite.noise_profile_set", &json!({"profile": null})).unwrap();
        assert!(e.noise_profile.is_none());
        assert!(e.execute("audiosuite.noise_profile_set", &json!({"profile": {"sample_rate": 48_000}})).is_err());
        let mut broken = data.clone();
        broken["fft_size"] = json!(16);
        assert!(e.execute("audiosuite.noise_profile_set", &json!({"profile": broken})).is_err());
        e.execute("audiosuite.noise_profile_set", &json!({"profile": data.clone()})).unwrap();
        assert!(e.noise_profile.is_some());

        let mut other = data;
        other["sample_rate"] = json!(44_100);
        e.execute("audiosuite.noise_profile_set", &json!({"profile": other})).unwrap();
        let err = e.execute("audiosuite.noise_reduce", &json!({"clips": [id.0]})).unwrap_err().to_string();
        assert!(err.contains("44100 Hz"), "{err}");
    }

    #[test]
    fn learning_over_a_gap_is_refused_and_keeps_the_profile() {
        let (mut e, t, _) = take();
        e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        let before = e.noise_profile.clone();
        // The take ends at 96 000: this passage is half silence after it.
        let err = e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 90_000, "end": 102_000})).unwrap_err().to_string();
        assert!(err.contains("gap"), "{err}");
        assert_eq!(e.noise_profile, before);
    }

    #[test]
    fn a_muted_clip_is_cleaned_from_its_audio_and_stays_muted() {
        let (mut e, t, id) = take();
        e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        e.session_mut().find_clip_mut(id).unwrap().muted = true;
        e.execute("audiosuite.noise_reduce", &json!({"clips": [id.0]})).unwrap();
        let mut s = e.session().clone();
        let copy = s.tracks[1].id;
        let c = s.tracks[1].clips()[0].clone();
        assert!(c.muted, "the copy keeps the clip's mute");
        s.find_clip_mut(c.id).unwrap().muted = false;
        let audio = soundcraft_mix::render_clips(&s, copy, Range::new(52_000, 80_000));
        assert!(rms(&audio[0]) > 0.2, "the tone was lost: {}", rms(&audio[0]));
    }

    #[test]
    fn named_tracks_limit_the_selected_clips() {
        let (mut e, t, id) = take();
        let other = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Other"));
        e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        e.session_mut().edit.selected_clips = vec![id];
        let err = e.execute("audiosuite.noise_reduce", &json!({"tracks": [other.0]})).unwrap_err().to_string();
        assert!(err.contains("select audio clips"), "{err}");
        assert_eq!(e.session().tracks.len(), 2, "nothing was copied");
    }

    #[test]
    fn the_copy_of_a_soloed_track_stays_soloed() {
        let (mut e, t, id) = take();
        e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        e.session_mut().track_mut(t).unwrap().mixer.solo = true;
        e.execute("audiosuite.noise_reduce", &json!({"clips": [id.0]})).unwrap();
        let s = e.session();
        assert!(s.tracks[0].mixer.solo && s.tracks[0].mixer.mute);
        assert!(s.tracks[1].mixer.solo && !s.tracks[1].mixer.mute, "the copy must still be heard");
    }

    #[test]
    fn crossfading_clips_are_both_copied() {
        let (mut e, t, id) = take();
        // A second clip of the take overlapping the first one's last 4800 samples, as a crossfade.
        let src = match e.session().find_clip(id).unwrap().1.content {
            ClipContent::Audio { source, .. } => source,
            _ => panic!("an audio clip"),
        };
        let id2 = e.session_mut().new_clip_id();
        let c2 = Clip::audio(id2, "take2", src, 0, 91_200, 48_000);
        e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c2);
        assert_eq!(e.session().tracks[0].clips().len(), 2);
        e.execute("audiosuite.noise_learn", &json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        e.execute("audiosuite.noise_reduce", &json!({"clips": [id.0, id2.0]})).unwrap();
        let copy: Vec<(i64, i64)> = e.session().tracks[1].clips().iter().map(|c| (c.start, c.length)).collect();
        assert_eq!(copy, vec![(0, 96_000), (91_200, 48_000)]);
    }
}
