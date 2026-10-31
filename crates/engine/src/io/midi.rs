use super::{ChannelFormat, Clip, ClipContent, Engine, EngineError, Result, Samples, TrackId, TrackKind};

/// Most meter changes a MIDI file may bring into the tempo map; scores have a handful.
pub(super) const MAX_MIDI_METERS: usize = 10_000;
/// Most tempo changes a MIDI file may bring into the tempo map: conversions walk the tempo events.
pub(super) const MAX_MIDI_TEMPOS: usize = 10_000;
/// Most synth tracks a MIDI file may create in the session.
pub(super) const MAX_MIDI_TRACKS: usize = 512;

/// Import a Standard MIDI File as MIDI tracks (one per SMF track).
pub fn import_midi_bytes(e: &mut Engine, bytes: &[u8], at: Samples, tempo_map: bool) -> Result<Vec<TrackId>> {
    let smf = soundcraft_midi::read_smf(bytes).map_err(|err| EngineError::Io(err.to_string()))?;
    if tempo_map {
        // Each meter is placed by walking the ones before it.
        if smf.meters.len() > MAX_MIDI_METERS {
            return Err(EngineError::Io(format!("MIDI file: more than {MAX_MIDI_METERS} meter changes")));
        }
        if smf.tempos.len() > MAX_MIDI_TEMPOS {
            return Err(EngineError::Io(format!("MIDI file: more than {MAX_MIDI_TEMPOS} tempo changes")));
        }
    }
    let tracks_to_import = smf.tracks.iter().filter(|tr| !tr.sequence.notes.is_empty() || !tr.sequence.ctrls.is_empty()).count();
    if tracks_to_import > MAX_MIDI_TRACKS {
        return Err(EngineError::Io(format!("MIDI file: more than {MAX_MIDI_TRACKS} tracks")));
    }
    let s = e.session_mut();
    if tempo_map {
        for (tick, bpm) in &smf.tempos {
            if s.tempo.tempo_at_tick(*tick) == *bpm {
                continue;
            }
            let _ = s.tempo.set_tempo(*tick, *bpm);
        }
        for (tick, n, d) in &smf.meters {
            let _ = s.tempo.set_meter(*tick, u32::from(*n), u32::from(*d));
        }
    }
    let base_tick = s.tempo.samples_to_ticks(at, s.sample_rate);
    let mut out = Vec::new();
    for (i, tr) in smf.tracks.iter().enumerate() {
        if tr.sequence.notes.is_empty() && tr.sequence.ctrls.is_empty() {
            continue;
        }
        let mut name = if tr.name.trim().is_empty() { format!("MIDI {}", i + 1) } else { tr.name.clone() };
        soundcraft_model::truncate_chars(&mut name, soundcraft_model::MAX_NAME_CHARS);
        let id = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
        let drums = !tr.sequence.notes.is_empty() && tr.sequence.notes.iter().all(|n| n.channel == 9);
        if let Some(t) = s.track_mut(id) {
            t.instrument = Some(soundcraft_model::Insert::new(if drums { "drum_synth" } else { "subtractive_synth" }));
        }
        // A controller on the clip's last tick would not play, so the clip runs one tick past the last one.
        let past_ctrls = tr.sequence.ctrls.iter().map(|c| c.tick.saturating_add(1)).max().unwrap_or(0);
        let end_tick = base_tick.saturating_add(tr.sequence.end_tick().max(past_ctrls).max(1));
        let len = s.tempo.tick_to_samples(end_tick, s.sample_rate).saturating_sub(at);
        if len > s.sample_rate.samples(86_400.0) {
            return Err(EngineError::Io(format!("MIDI track {}: longer than 24 hours", i + 1)));
        }
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, id, Clip::midi(cid, name, at, len.max(1), tr.sequence.clone()));
        out.push(id);
    }
    Ok(out)
}

/// Export MIDI clips as a format-1 SMF. `only` limits the file to those tracks.
pub fn export_midi(e: &Engine, only: Option<&[TrackId]>) -> Result<Vec<u8>> {
    let s = e.session();
    let mut smf = soundcraft_midi::Smf {
        format: 1,
        tracks: Vec::new(),
        tempos: s.tempo.tempos().iter().map(|t| (t.tick, t.bpm)).collect(),
        meters: s.tempo.meters().iter().map(|m| (m.tick, u8::try_from(m.numerator).unwrap_or(4), u8::try_from(m.denominator).unwrap_or(4))).collect(),
        markers: s.markers.iter().map(|m| (s.tempo.samples_to_ticks(m.start, s.sample_rate), m.name.clone())).collect(),
        key_sigs: Vec::new(),
    };
    let chosen: Vec<_> = s.tracks.iter().filter(|t| t.kind.is_midi() && only.is_none_or(|ids| ids.contains(&t.id))).collect();
    if only.is_some() && chosen.is_empty() {
        return Err(EngineError::BadParams("file.export_midi".into(), "none of the requested tracks hold MIDI".into()));
    }
    for t in chosen {
        let mut seq = soundcraft_midi::Sequence::default();
        for c in t.clips() {
            if let ClipContent::Midi { sequence } = &c.content {
                let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
                for n in &sequence.notes {
                    let mut n = *n;
                    n.start = n.start.saturating_add(base);
                    seq.notes.push(n);
                }
                for ev in &sequence.ctrls {
                    let mut ev = *ev;
                    ev.tick = ev.tick.saturating_add(base);
                    seq.ctrls.push(ev);
                }
            }
        }
        seq.sort();
        smf.tracks.push(soundcraft_midi::SmfTrack { name: t.name.clone(), sequence: seq });
    }
    soundcraft_midi::write_smf(&smf).map_err(|err| EngineError::Io(err.to_string()))
}
