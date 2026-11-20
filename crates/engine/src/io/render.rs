use super::import::add_source;
#[cfg(test)]
use super::midi::{MAX_MIDI_METERS, MAX_MIDI_TEMPOS, MAX_MIDI_TRACKS, import_midi_bytes};
use super::{AudioBuffer, Clip, ClipContent, Engine, EngineError, FileFormat, Range, Result, Samples, Session, TrackId, TrackKind};

/// Consolidate `r` on a track into one new clip backed by a rendered source. The clip covers the
/// whole range, silence included; a range too long to render is refused rather than shortened.
pub fn consolidate(e: &mut Engine, cmd: &str, t: TrackId, r: Range) -> Result<bool> {
    let s = e.session();
    let Some(tr) = s.track(t).filter(|tr| tr.kind.has_playlist() && !tr.kind.is_midi()) else { return Ok(false) };
    let start = r.start.max(0);
    let len = r.end.saturating_sub(start);
    if len <= 0 {
        return Ok(false);
    }
    let max = soundcraft_mix::MAX_RENDER_SAMPLES / tr.channels().max(1);
    if usize::try_from(len).map_or(true, |n| n > max) {
        let secs = max as f64 / s.sample_rate.as_f64();
        return Err(crate::cmd::bad(cmd, format!("selection is too long to consolidate on `{}` (at most {secs:.0} s)", tr.name)));
    }
    let r = Range::new(start, start + len);
    let audio = soundcraft_mix::render_clips(s, t, r);
    if audio.iter().any(|ch| ch.len() as i64 != len) {
        return Err(EngineError::Internal(cmd.to_string(), "consolidate rendered the wrong length".into()));
    }
    let name = format!("{}_consolidated", tr.name);
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &name, buf, None, FileFormat::Wav);
    crate::edit::clear_range(s, t, r, false);
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::audio(cid, name, src, 0, r.start, r.len()));
    Ok(true)
}

/// The most frames of `channels`-channel audio one offline render holds.
pub fn render_limit(channels: usize) -> usize {
    soundcraft_mix::MAX_RENDER_SAMPLES / channels.max(1)
}

/// Refuses `len` frames of `name` past `max`, where a render would cut the audio short.
pub fn refuse_longer(s: &Session, cmd: &str, name: &str, verb: &str, len: Samples, max: usize) -> Result<()> {
    if len <= Samples::try_from(max).unwrap_or(Samples::MAX) {
        return Ok(());
    }
    let secs = max as f64 / s.sample_rate.as_f64();
    Err(crate::cmd::bad(cmd, format!("`{name}` is too long to {verb} (at most {secs:.0} s)")))
}

/// Render clip gain into a new source and reset the clip's gain.
pub fn render_clip_gain(e: &mut Engine, cmd: &str, id: soundcraft_model::ClipId) -> Result<bool> {
    let s = e.session();
    let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { return Ok(false) };
    if !c.is_audio() {
        return Ok(false);
    }
    refuse_longer(s, cmd, &c.name, "render", c.length, render_limit(s.track(t).map_or(1, |x| x.channels())))?;
    let mut c2 = c.clone();
    c2.fade_in = Default::default();
    c2.fade_out = Default::default();
    let mut tmp = s.clone();
    if let Some(pl) = tmp.track_mut(t).and_then(|x| x.playlist_mut()) {
        pl.clips = vec![c2];
    }
    let audio = soundcraft_mix::render_clips(&tmp, t, c.range());
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &format!("{}_gain", c.name), buf, None, FileFormat::Wav);
    if let Some(cl) = s.find_clip_mut(id) {
        cl.content = ClipContent::Audio { source: src, offset: 0 };
        cl.gain_db = 0.0;
        cl.gain_env.clear();
        // The new source is the clip as it already sounds, stretch included.
        // Keeping the old ratio would play that audio through the stretch again.
        cl.stretch = 1.0;
    }
    Ok(true)
}

/// Commit a track's inserts into a new audio track (Track › Commit).
pub fn commit_track(e: &mut Engine, t: TrackId) -> Result<Option<TrackId>> {
    let s = e.session();
    let Some(tr) = s.track(t) else { return Ok(None) };
    if !tr.kind.is_audio_path() || tr.kind == TrackKind::Master {
        return Ok(None);
    }
    let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0);
    if end <= 0 {
        return Ok(None);
    }
    let fmt = tr.format;
    if fmt.channels() > soundcraft_mix::MAX_CHANNELS {
        return Err(EngineError::BadParams(
            "track.commit".into(),
            format!("track commit supports at most {} channels; cannot render {}", soundcraft_mix::MAX_CHANNELS, fmt.label()),
        ));
    }
    let too_long = || EngineError::BadParams("track.commit".into(), format!("`{}` is too long to commit within the offline render limit", tr.name));
    let end = end.checked_add(s.sample_rate.samples(2.0)).ok_or_else(too_long)?;
    let frames = usize::try_from(end).map_err(|_| too_long())?;
    // The shared renderer uses a stereo main mix for mono, then folds it back to mono.
    // Reject requests it would truncate before rendering or replacing the source track.
    if frames > soundcraft_mix::MAX_RENDER_SAMPLES / fmt.channels().max(2) {
        return Err(too_long());
    }
    let range = Range::new(0, end);
    let audio = soundcraft_mix::render_track_pre_fader(s, t, range);
    if audio.len() != fmt.channels() || audio.iter().any(|channel| channel.len() != frames) {
        return Err(EngineError::Io("track commit did not render the complete channel format and range".into()));
    }
    let name = format!("{}.cm", tr.name);
    let mixer = tr.mixer.clone();
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &name, buf, None, FileFormat::Wav);
    let nt = s.add_track(TrackKind::Audio, fmt, Some(&name));
    if let (Some(idx), Some(new_idx)) = (s.track_index(t), s.track_index(nt)) {
        let tr = s.tracks.remove(new_idx);
        s.tracks.insert((idx + 1).min(s.tracks.len()), tr);
    }
    let cid = s.new_clip_id();
    if let Some(ntr) = s.track_mut(nt) {
        ntr.mixer.volume_db = mixer.volume_db;
        ntr.mixer.output = mixer.output.clone();
        ntr.mixer.sends = mixer.sends.clone();
        if let Some(pl) = ntr.playlist_mut() {
            pl.clips.push(Clip::audio(cid, name, src, 0, 0, range.len()));
        }
    }
    if let Some(old) = s.track_mut(t) {
        old.inactive = true;
        old.hidden = true;
    }
    Ok(Some(nt))
}

pub fn sanitize_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "untitled".into() } else { s.chars().take(100).collect() }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn a_midi_track_name_is_cut_to_a_length_that_can_be_drawn() {
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let smf = soundcraft_midi::Smf { tracks: vec![soundcraft_midi::SmfTrack { name: "n".repeat(1_000_000), sequence }], ..Default::default() };
        let mut e = Engine::default();
        let ids = import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf).unwrap(), 0, false).unwrap();
        let track = e.session().track(ids[0]).unwrap();
        assert_eq!((track.name.len(), track.clips()[0].name.len()), (soundcraft_model::MAX_NAME_CHARS, soundcraft_model::MAX_NAME_CHARS));
    }

    #[test]
    fn a_controller_only_midi_track_is_imported_with_its_events() {
        let ctrls = vec![
            soundcraft_midi::CtrlEvent { tick: 0, channel: 0, kind: soundcraft_midi::CtrlKind::Cc(7), value: 100 },
            soundcraft_midi::CtrlEvent { tick: 480, channel: 0, kind: soundcraft_midi::CtrlKind::Cc(7), value: 64 },
            soundcraft_midi::CtrlEvent { tick: 960, channel: 0, kind: soundcraft_midi::CtrlKind::Cc(7), value: 20 },
        ];
        let sequence = soundcraft_midi::Sequence { ctrls: ctrls.clone(), ..Default::default() };
        let smf = soundcraft_midi::Smf { tracks: vec![soundcraft_midi::SmfTrack { name: "Vol".into(), sequence }], ..Default::default() };
        let mut e = Engine::default();
        let ids = import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf).unwrap(), 0, false).unwrap();
        assert_eq!(ids.len(), 1);
        let track = e.session().track(ids[0]).unwrap();
        assert_eq!(track.instrument.as_ref().map(|i| i.plugin.as_str()), Some("subtractive_synth"));
        // The clip must reach past the last event, or playback skips it.
        assert!(track.clips()[0].end() > e.session().tempo.tick_to_samples(960, e.session().sample_rate));
        match &track.clips()[0].content {
            ClipContent::Midi { sequence } => assert_eq!(sequence.ctrls, ctrls),
            _ => panic!("expected a MIDI clip"),
        }
    }

    #[test]
    fn a_midi_file_with_stray_bytes_after_its_last_track_is_imported() {
        // #1528: a complete format-0 file followed by two line feeds, too short to be a chunk.
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let smf = soundcraft_midi::Smf { format: 0, tracks: vec![soundcraft_midi::SmfTrack { name: String::new(), sequence }], ..Default::default() };
        let mut bytes = soundcraft_midi::write_smf(&smf).unwrap();
        bytes.extend_from_slice(b"\n\n");
        let mut e = Engine::default();
        let ids = import_midi_bytes(&mut e, &bytes, 0, true).unwrap();
        assert_eq!(ids.len(), 1);
        match &e.session().track(ids[0]).unwrap().clips()[0].content {
            ClipContent::Midi { sequence } => assert_eq!(sequence.notes, vec![note]),
            _ => panic!("expected a MIDI clip"),
        }
        // Something that is no MIDI file is still refused, and leaves the session as it was.
        let mut e = Engine::default();
        assert!(import_midi_bytes(&mut e, b"RIFF\0\0\0\0WAVE", 0, true).is_err());
        assert!(e.session().tracks.is_empty());
    }

    #[test]
    fn a_midi_file_past_the_track_limit_is_refused_even_with_stray_bytes() {
        // Read leniently, the tracks past the limit would be left out without a word: the note in
        // the last one would be lost. The file stays refused, and the session as it was.
        let tracks = soundcraft_midi::MAX_TRACKS + 1;
        let mut bytes = b"MThd\0\0\0\x06\0\x01".to_vec();
        bytes.extend_from_slice(&u16::try_from(tracks).unwrap().to_be_bytes());
        bytes.extend_from_slice(&960u16.to_be_bytes());
        for _ in 1..tracks {
            bytes.extend_from_slice(b"MTrk\0\0\0\x04\0\xFF\x2F\0");
        }
        // A note on, a quarter note later its note off, then End of Track.
        bytes.extend_from_slice(b"MTrk\0\0\0\x0D\0\x90\x3C\x64\x87\x40\x80\x3C\x40\0\xFF\x2F\0");
        bytes.extend_from_slice(b"\n\n");
        let mut e = Engine::default();
        let err = import_midi_bytes(&mut e, &bytes, 0, true).unwrap_err();
        assert!(err.to_string().contains("too many tracks"), "{err}");
        assert!(e.session().tracks.is_empty());
    }

    #[test]
    fn a_midi_file_past_the_event_limit_is_refused_after_a_damaged_track() {
        // The strict reader stops at track 1's damage, before track 2 reaches the event limit; read
        // leniently, track 2's note after the limit (pitch 72) would be lost without a word.
        let mut damaged = b"\0\x3C\x40".to_vec(); // a data byte with no status before it
        damaged.extend_from_slice(b"\0\xFF\x2F\0");
        let mut long = b"\0\x90\x3C\x64".to_vec();
        for _ in 0..soundcraft_midi::MAX_EVENTS {
            long.extend_from_slice(b"\0\x3C\0"); // running status: note off
        }
        long.extend_from_slice(b"\0\x90\x48\x64\x87\x40\x80\x48\x40\0\xFF\x2F\0");
        let mut bytes = b"MThd\0\0\0\x06\0\x01\0\x02\x03\xC0".to_vec();
        for body in [&damaged, &long] {
            bytes.extend_from_slice(b"MTrk");
            bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
            bytes.extend_from_slice(body);
        }
        assert!(!matches!(soundcraft_midi::read_smf(&bytes), Err(soundcraft_midi::MidiError::TooManyEvents(_)) | Ok(_)));
        let mut e = Engine::default();
        let err = import_midi_bytes(&mut e, &bytes, 0, true).unwrap_err();
        assert!(err.to_string().contains("events"), "{err}");
        assert!(e.session().tracks.is_empty());
    }

    #[test]
    fn midi_longer_than_a_day_is_refused() {
        let held = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: (1 << 28) - 1 };
        let sequence = soundcraft_midi::Sequence { notes: vec![held], ..Default::default() };
        let smf = soundcraft_midi::Smf { tracks: vec![soundcraft_midi::SmfTrack { name: String::new(), sequence }], ..Default::default() };
        let err = import_midi_bytes(&mut Engine::default(), &soundcraft_midi::write_smf(&smf).unwrap(), 0, false).unwrap_err();
        assert!(err.to_string().contains("longer than 24 hours"), "{err}");
    }

    #[test]
    fn a_refused_midi_file_leaves_the_session_untouched() {
        // Track 1 fits; track 2 is longer than a day. Before the import is refused, nothing of
        // track 1, nor the file's tempo and meter changes, may have reached the session.
        let note = |length| soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length };
        let short = soundcraft_midi::Sequence { notes: vec![note(960)], ..Default::default() };
        let held = soundcraft_midi::Sequence { notes: vec![note((1 << 28) - 1)], ..Default::default() };
        let smf = soundcraft_midi::Smf {
            tracks: vec![
                soundcraft_midi::SmfTrack { name: "fits".into(), sequence: short },
                soundcraft_midi::SmfTrack { name: "too long".into(), sequence: held },
            ],
            tempos: vec![(0, 91.0)],
            meters: vec![(0, 7, 8)],
            ..Default::default()
        };
        let mut e = Engine::default();
        let before = e.session().clone();
        let (revision, dirty) = (e.revision, e.is_dirty());
        let err = import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf).unwrap(), 0, true).unwrap_err();
        assert!(err.to_string().contains("MIDI track 2: longer than 24 hours"), "{err}");
        assert_eq!(e.session().tracks.len(), before.tracks.len());
        assert_eq!(e.session().tempo, before.tempo);
        assert_eq!(*e.session(), before);
        assert_eq!((e.revision, e.is_dirty()), (revision, dirty), "a refused file is not an edit");
    }

    #[test]
    fn midi_with_more_meter_changes_than_scores_have_is_refused() {
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let meters = (0..=MAX_MIDI_METERS as i64).map(|bar| (bar * 3840, 4, 4)).collect();
        let smf = soundcraft_midi::Smf { tracks: vec![soundcraft_midi::SmfTrack { name: String::new(), sequence }], meters, ..Default::default() };
        let err = import_midi_bytes(&mut Engine::default(), &soundcraft_midi::write_smf(&smf).unwrap(), 0, true).unwrap_err();
        assert!(err.to_string().contains("meter changes"), "{err}");
    }

    /// A MIDI file with more tempo changes than MAX_MIDI_TEMPOS is refused without mutating the session.
    #[test]
    fn midi_with_more_tempo_changes_than_performances_have_is_refused() {
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let smf = |n: usize| soundcraft_midi::Smf {
            tracks: vec![soundcraft_midi::SmfTrack { name: String::new(), sequence: sequence.clone() }],
            tempos: (0..n as i64).map(|beat| (beat * 960, 100.0 + (beat % 40) as f64)).collect(),
            ..Default::default()
        };
        let mut e = Engine::default();
        let err = import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf(MAX_MIDI_TEMPOS + 1)).unwrap(), 0, true).unwrap_err();
        assert!(err.to_string().contains("tempo changes"), "{err}");
        assert_eq!((e.session().tempo.tempos().len(), e.session().tracks.len()), (1, 0), "nothing is imported");
        import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf(MAX_MIDI_TEMPOS)).unwrap(), 0, true).unwrap();
        assert_eq!(e.session().tempo.tempos().len(), MAX_MIDI_TEMPOS);
    }

    /// A MIDI file with more tracks than MAX_MIDI_TRACKS is refused without mutating the session.
    #[test]
    fn midi_with_more_tracks_than_supported_is_refused() {
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let smf = |n: usize| soundcraft_midi::Smf {
            tracks: (0..n).map(|i| soundcraft_midi::SmfTrack { name: format!("Tr {i}"), sequence: sequence.clone() }).collect(),
            ..Default::default()
        };
        let mut e = Engine::default();
        let err = import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf(MAX_MIDI_TRACKS + 1)).unwrap(), 0, false).unwrap_err();
        assert!(err.to_string().contains("tracks"), "{err}");
        assert_eq!(e.session().tracks.len(), 0, "no tracks imported");
        import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf(MAX_MIDI_TRACKS)).unwrap(), 0, false).unwrap();
        assert_eq!(e.session().tracks.len(), MAX_MIDI_TRACKS);
    }

    /// Consecutive equal tempo events in a MIDI file are collapsed to actual tempo changes.
    #[test]
    fn midi_collapses_equal_consecutive_tempos() {
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let smf = soundcraft_midi::Smf {
            tracks: vec![soundcraft_midi::SmfTrack { name: String::new(), sequence }],
            tempos: vec![(0, 120.0), (960, 120.0), (1920, 140.0), (2880, 140.0), (3840, 100.0)],
            ..Default::default()
        };
        let mut e = Engine::default();
        import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf).unwrap(), 0, true).unwrap();
        let tempos = e.session().tempo.tempos();
        assert_eq!(tempos.len(), 3);
        assert!((tempos[0].bpm - 120.0).abs() < 1e-3);
        assert!((tempos[1].bpm - 140.0).abs() < 1e-3);
        assert!((tempos[2].bpm - 100.0).abs() < 1e-3);
    }

    /// A MIDI file replaces an existing session tempo event even if the MIDI file has consecutive equal tempos.
    #[test]
    fn midi_import_replaces_differing_existing_session_tempo() {
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let smf = soundcraft_midi::Smf {
            tracks: vec![soundcraft_midi::SmfTrack { name: String::new(), sequence }],
            tempos: vec![(0, 120.0), (960, 120.0)],
            ..Default::default()
        };
        let mut e = Engine::default();
        e.session_mut().tempo.set_tempo(960, 80.0).unwrap();
        assert_eq!(e.session().tempo.tempo_at_tick(960), 80.0);
        import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf).unwrap(), 0, true).unwrap();
        assert_eq!(e.session().tempo.tempo_at_tick(960), 120.0);
    }

    #[test]
    fn new_sources_take_the_first_free_file_name() {
        let mut s = Session::default();
        let buf = || AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 4]] };
        add_source(&mut s, "k", buf(), Some("Audio Files/k-02.wav"), FileFormat::Wav);
        let mut path = || {
            let id = add_source(&mut s, "k", buf(), None, FileFormat::Wav);
            s.source(id).map(|x| x.path.clone()).unwrap_or_default()
        };
        assert_eq!([path(), path(), path()], ["Audio Files/k.wav", "Audio Files/k-2.wav", "Audio Files/k-3.wav"]);
    }
}
