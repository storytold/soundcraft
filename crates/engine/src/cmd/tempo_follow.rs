//! Tick-timebase tracks follow the tempo map: when the tempo changes, their clips keep their
//! musical positions instead of their sample positions.

use super::*;
use soundcraft_model::{ClipContent, Session};
use soundcraft_time::TempoMap;

/// Run a tempo edit, then re-anchor the clips on tick-timebase tracks to the new tempo map.
pub(super) fn following_tempo(e: &mut Engine, edit: impl FnOnce(&mut Engine) -> Result<Value>) -> Result<Value> {
    let old = e.session().tempo.clone();
    let out = edit(e)?;
    follow_tempo(e.session_mut(), &old)?;
    Ok(out)
}

/// Move the clips of every tick-timebase track from where `old` put them to where the session's
/// tempo map puts the same musical positions. MIDI clips and audio on Elastic Audio tracks also
/// keep their length in ticks: audio is re-stretched to fit, and Varispeed tracks change speed.
/// Audio on other tracks keeps its length in samples.
pub(super) fn follow_tempo(s: &mut Session, old: &TempoMap) -> Result<()> {
    if *old == s.tempo {
        return Ok(());
    }
    let sr = s.sample_rate;
    let new = s.tempo.clone();
    // (clip, new length) for audio that must be stretched once every clip has moved.
    let mut stretch: Vec<(ClipId, Samples, bool)> = Vec::new();
    for tr in s.tracks.iter_mut().filter(|t| t.ticks_timebase && t.kind.has_playlist()) {
        let algorithm = tr.elastic.clone();
        for pl in &mut tr.playlists {
            for c in pl.clips.iter_mut().filter(|c| !c.time_locked) {
                let t0 = old.samples_to_ticks_f(c.start, sr);
                let t1 = old.samples_to_ticks_f(c.start.saturating_add(c.length), sr);
                let start = new.ticks_f_to_samples(t0, sr).max(0);
                let length = (new.ticks_f_to_samples(t1, sr) - start).max(1);
                c.start = start;
                match (&c.content, algorithm.as_deref()) {
                    (ClipContent::Midi { .. }, _) => c.length = length,
                    (ClipContent::Audio { .. }, Some(alg)) if length != c.length => stretch.push((c.id, length, alg == "varispeed")),
                    _ => {}
                }
            }
            pl.sort();
        }
    }
    for (id, length, varispeed) in stretch {
        if varispeed {
            if let Some(c) = s.find_clip_mut(id) {
                c.stretch = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 } * length as f64 / c.length.max(1) as f64;
                c.length = length;
                c.fade_in.len = c.fade_in.len.min(length);
                c.fade_out.len = c.fade_out.len.min(length);
            }
        } else {
            // A stretch that cannot render fails the command, which restores the document, rather
            // than leaving moved clips overlapping.
            super::clip::warp_clip(s, id, |base| length as f64 / base.length as f64, true)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, SourceId, TrackKind};

    const SOURCE_LEN: usize = 96_000;

    #[test]
    fn tick_audio_clips_follow_a_slower_tempo() {
        let (mut e, _, ids) = engine_with_clips(Some("rhythmic"));
        let before: Vec<(Samples, Samples)> = ids.iter().map(|id| span(&e, *id)).collect();
        e.execute("event.tempo", &json!({"bpm": 60.0})).unwrap();
        for (id, (start, length)) in ids.iter().zip(before) {
            let (new_start, new_length) = span(&e, *id);
            // 120 → 60 bpm: everything takes twice as long.
            assert!((new_start - start * 2).abs() <= 2, "start {start} became {new_start}");
            assert!((new_length - length * 2).abs() <= 2, "length {length} became {new_length}");
        }
    }

    #[test]
    fn elastic_clips_do_not_overlap_after_a_tempo_change() {
        let (mut e, _, ids) = engine_with_clips(Some("polyphonic"));
        e.execute("event.tempo", &json!({"bpm": 54.0})).unwrap();
        let (a_start, a_len) = span(&e, ids[0]);
        let (b_start, _) = span(&e, ids[1]);
        assert!(a_start + a_len <= b_start + 2, "first clip ends at {} after the second starts at {b_start}", a_start + a_len);
        assert_eq!(e.session().find_clip(ids[1]).unwrap().1.start, b_start);
    }

    #[test]
    fn varispeed_clips_change_speed_with_the_tempo() {
        let (mut e, _, ids) = engine_with_clips(Some("varispeed"));
        let (_, length) = span(&e, ids[0]);
        e.execute("event.tempo", &json!({"bpm": 60.0})).unwrap();
        let c = e.session().find_clip(ids[0]).unwrap().1.clone();
        assert!((c.length - length * 2).abs() <= 2);
        assert!((c.stretch - 2.0).abs() < 1e-3, "stretch {}", c.stretch);
    }

    #[test]
    fn tick_clips_without_elastic_audio_move_but_keep_their_length() {
        let (mut e, _, ids) = engine_with_clips(None);
        let (start, length) = span(&e, ids[1]);
        e.execute("event.tempo", &json!({"bpm": 60.0})).unwrap();
        let (new_start, new_length) = span(&e, ids[1]);
        assert!((new_start - start * 2).abs() <= 2);
        assert_eq!(new_length, length);
    }

    #[test]
    fn sample_timebase_tracks_stay_put() {
        let (mut e, t, ids) = engine_with_clips(Some("rhythmic"));
        e.session_mut().track_mut(t).unwrap().ticks_timebase = false;
        let before = span(&e, ids[1]);
        e.execute("event.tempo", &json!({"bpm": 60.0})).unwrap();
        assert_eq!(span(&e, ids[1]), before);
    }

    #[test]
    fn a_dragged_tempo_does_not_degrade_the_audio_each_step() {
        let (mut e, _, ids) = engine_with_clips(Some("rhythmic"));
        for bpm in [118.0, 110.0, 90.0, 70.0, 54.0] {
            e.execute("event.tempo", &json!({"bpm": bpm})).unwrap();
        }
        let (_, length) = span(&e, ids[0]);
        let expected = (SOURCE_LEN as f64 / 2.0 * 120.0 / 54.0).round() as Samples;
        assert!((length - expected).abs() <= 4, "length {length}, expected {expected}");
        // Every step stretched the original, not the previous render.
        e.execute("clip.remove_warp", &json!({"clip": ids[0].0})).unwrap();
        assert_eq!(span(&e, ids[0]).1, SOURCE_LEN as Samples / 2);
    }

    #[test]
    fn removing_a_tempo_change_brings_the_clips_back() {
        let (mut e, _, ids) = engine_with_clips(Some("rhythmic"));
        let before: Vec<(Samples, Samples)> = ids.iter().map(|id| span(&e, *id)).collect();
        e.execute("event.tempo", &json!({"bpm": 60.0, "at": 12_000})).unwrap();
        assert_ne!(span(&e, ids[1]), before[1]);
        e.execute("event.tempo_remove", &json!({"at": 12_000})).unwrap();
        for (id, (start, length)) in ids.iter().zip(before) {
            let (new_start, new_length) = span(&e, *id);
            assert!((new_start - start).abs() <= 2, "start {start} became {new_start}");
            assert!((new_length - length).abs() <= 2, "length {length} became {new_length}");
        }
    }

    #[test]
    fn identifying_the_beat_moves_the_clips_with_the_new_tempo() {
        let (mut e, _, ids) = engine_with_clips(None);
        let (start, _) = span(&e, ids[1]);
        e.execute("event.identify_beat", &json!({"start": 0, "end": 192_000, "bars": 1})).unwrap();
        // One bar over four seconds is 60 bpm, half the demo's 120.
        assert!((span(&e, ids[1]).0 - start * 2).abs() <= 2);
    }

    #[test]
    fn a_stretch_that_cannot_render_refuses_the_tempo_change() {
        let (mut e, _, ids) = engine_with_clips(Some("rhythmic"));
        let (tempo, before) = (e.session().tempo.clone(), ids.iter().map(|id| span(&e, *id)).collect::<Vec<_>>());
        // 120 → 5 bpm would stretch the audio 24 times, past what the renderer supports.
        assert!(e.execute("event.tempo", &json!({"bpm": 5.0})).is_err());
        assert!(e.session().tempo == tempo);
        assert_eq!(ids.iter().map(|id| span(&e, *id)).collect::<Vec<_>>(), before);
    }

    fn span(e: &Engine, id: ClipId) -> (Samples, Samples) {
        let c = e.session().find_clip(id).unwrap().1;
        (c.start, c.length)
    }

    /// Two back-to-back half-second clips (at 120 bpm: one bar) on a tick-timebase audio track.
    fn engine_with_clips(algorithm: Option<&str>) -> (Engine, TrackId, Vec<ClipId>) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let tone: Vec<f32> = (0..SOURCE_LEN).map(|i| (i as f32 * 0.05).sin() * 0.4).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
        let src: SourceId = crate::io::add_source(&mut s, "tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let half = SOURCE_LEN as Samples / 2;
        let mut ids = Vec::new();
        for i in 0..2 {
            let id = s.new_clip_id();
            crate::edit::place_clip(&mut s, t, soundcraft_model::Clip::audio(id, "tone", src, i * half, i * half, half));
            ids.push(id);
        }
        if let Some(tr) = s.track_mut(t) {
            tr.ticks_timebase = true;
            tr.elastic = algorithm.map(str::to_string);
        }
        (Engine::new(s), t, ids)
    }
}
