//! MIDI played during a record pass, turned into MIDI clips.

use super::{Clip, Engine, Range, Samples, Session, TrackId, TrackKind};
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{ClipId, MidiInput, Route};
use soundcraft_time::{SampleRate, TempoMap};
use std::collections::HashMap;
use std::sync::Arc;

/// A MIDI message played during a record pass, at its timeline position; or, with the status
/// [`MidiTakeEvent::LOST`], the point where the input queues dropped events; or, with
/// [`MidiTakeEvent::WRAP`], the point where loop recording went back to the start of the loop.
/// Entries are in the order they were played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiTakeEvent {
    pub at: Samples,
    /// Index into the take's device names.
    pub device: u8,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

impl MidiTakeEvent {
    /// The `status` of an entry that marks dropped events: events of `device` (of every device
    /// when it is `u8::MAX`) were lost at `at`. Their note-offs may be among them, so the notes
    /// that device held end there.
    pub const LOST: u8 = 0xFF;

    /// The `status` of an entry that says loop recording went round: the entries after it were
    /// played in the next round of the loop. A key held across it sounds to the end of the loop
    /// and on from its start, also when its release is later on the timeline than its press.
    pub const WRAP: u8 = 0xFE;
}

/// How a record pass went over its range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MidiPass {
    /// Once, from its start to its end. Positions only go forward: a release that is stamped
    /// earlier than its key press (the audio callback ran late) ends the note at the press.
    #[default]
    Linear,
    /// Loop recording: over the range again and again. `stopped` is where inside the loop the
    /// pass stopped: the keys still down end there. When that is not known they count as held
    /// to the end of the range. `marked`: the take has a [`MidiTakeEvent::WRAP`] entry at every
    /// round, as the audio engine writes them, and only those say that the loop went round (a
    /// release stamped before its press in one round is a late callback, as in a linear pass).
    /// A take without those entries is not `marked`: there a release earlier on the timeline
    /// than its key press is read as the loop having gone round.
    Loop { stopped: Option<Samples>, marked: bool },
}

/// What a record pass made of the MIDI that was played.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiTake {
    /// The clips made, one per track that received something.
    pub clips: Vec<ClipId>,
    /// Places within the pass where input was dropped ([`MidiTakeEvent::LOST`]): the take is
    /// incomplete when this is not zero.
    pub gaps: usize,
}

mod projection;
pub(crate) use projection::ClockTakeProjection;

impl crate::ClockTake {
    /// Keep the events that actually start in this half-open timeline range. The external
    /// grid may have changed since capture, so a score-tick boundary alone cannot decide
    /// which piece owns an attack. Rebase the played ticks directly to avoid a second rounding.
    pub(crate) fn slice_events(
        &self,
        sequence: &Sequence,
        source: Range,
        range: Range,
        time: crate::edit::TimeCtx<'_>,
        grid: &TempoMap,
    ) -> (Sequence, Self) {
        let sr = time.sr;
        let score_origin = time.tempo.samples_to_ticks(source.start, sr);
        let played_origin = grid.samples_to_ticks(source.start, sr);
        let score_base = time.tempo.samples_to_ticks(range.start, sr);
        let played_base = grid.samples_to_ticks(range.start, sr);
        let played_sample = |tick: i64| grid.tick_to_samples(played_origin.saturating_add(tick), sr);
        let relative = |tick: i64, origin: i64, base: i64| origin.saturating_add(tick).saturating_sub(base).max(0);
        let score_tick = |at: Samples| time.tempo.samples_to_ticks(at, sr).saturating_sub(score_base).max(0);
        let select = |score: i64, played: Option<i64>| {
            let at = time.tempo.tick_to_samples(score_origin.saturating_add(score), sr);
            let heard = played.map(played_sample).unwrap_or(at);
            // An attack outside the original clip is silent at this tempo. Keep it in
            // exactly one score fragment, with its played tick still beyond the clip.
            let owner = if source.contains(heard) { heard } else { at };
            range.contains(owner).then(|| {
                // When a tempo change puts the two representations on opposite sides of
                // the cut, place the editable event at its new fragment's audible position.
                if range.contains(at) { (relative(score, score_origin, score_base), false) } else { (score_tick(heard), true) }
            })
        };
        let (played_notes, played_ctrls) = self.project_events(sequence);
        let notes = sequence
            .notes
            .iter()
            .copied()
            .zip(played_notes)
            .filter_map(|(mut n, played)| {
                let (start, from_played) = select(n.start, played.map(|p| p.start))?;
                n.start = start;
                let played = played.map(|mut p| {
                    if from_played {
                        n.length = score_tick(played_sample(p.end())).saturating_sub(start).max(1);
                    }
                    p.start = relative(p.start, played_origin, played_base);
                    p
                });
                Some((n, played))
            })
            .collect();
        let ctrls = sequence
            .ctrls
            .iter()
            .copied()
            .zip(played_ctrls)
            .filter_map(|(mut c, played)| {
                c.tick = select(c.tick, played.map(|p| p.tick))?.0;
                let played = played.map(|mut p| {
                    p.tick = relative(p.tick, played_origin, played_base);
                    p
                });
                Some((c, played))
            })
            .collect();
        Self::from_events(time, grid, range, notes, ctrls)
    }
}

/// Whether a record pass would record MIDI: a record-enabled MIDI or instrument track exists.
pub fn records_midi(e: &Engine) -> bool {
    e.session().tracks.iter().any(records_midi_on)
}

fn records_midi_on(t: &soundcraft_model::Track) -> bool {
    t.mixer.record_arm && !t.inactive && matches!(t.kind, TrackKind::Instrument | TrackKind::Midi)
}

/// The MIDI kinds Setup › MIDI › Input Filter keeps out of recordings.
fn midi_filtered(s: &Session, status: u8) -> bool {
    let kind = match status & 0xF0 {
        0x80 | 0x90 => "notes",
        0xA0 => "poly_aftertouch",
        0xB0 => "cc",
        0xC0 => "program",
        0xD0 => "mono_aftertouch",
        0xE0 => "pitch_bend",
        _ => return true,
    };
    !s.edit.flags.is_empty() && s.edit.flag(&format!("midi.filter.{kind}"))
}

/// Whether a [`MidiTakeEvent::LOST`] entry for `lost` covers `device`.
fn lost_covers(lost: u8, device: u8) -> bool {
    lost == u8::MAX || lost == device
}

/// What `track` recorded of `events` over `range`, in ticks from the start of the range. A key
/// already down when the range starts is left out; one still down at its end ends there, and one
/// whose device lost input ends where that happened. Keys are told apart by device, channel and
/// pitch: the same pitch held on two devices gives two notes. A key held while loop recording
/// goes round ([`MidiTakeEvent::WRAP`]) is two notes: to the end of the loop, and from its start;
/// that holds for a key still down when the pass stops too ([`MidiPass::Loop`]). In a pass that
/// did not loop ([`MidiPass::Linear`]) nothing is read as the loop going round.
fn midi_take_sequence(s: &Session, input: &MidiInput<'_>, range: Range, pass: MidiPass, events: &[MidiTakeEvent], devices: &[String]) -> Sequence {
    midi_take_on_grid(s, &s.tempo, input, range, pass, events, devices)
}
fn midi_take_on_grid(
    s: &Session,
    tempo: &soundcraft_time::TempoMap,
    input: &MidiInput<'_>,
    range: Range,
    pass: MidiPass,
    events: &[MidiTakeEvent],
    devices: &[String],
) -> Sequence {
    let sr = s.sample_rate;
    let base = tempo.samples_to_ticks(range.start, sr);
    let tick = |at: Samples| tempo.samples_to_ticks(at, sr).saturating_sub(base).max(0);
    let end_tick = tick(range.end).max(1);
    let looped = matches!(pass, MidiPass::Loop { .. });
    // Whether a release earlier on the timeline than its press says that the loop went round.
    let inferred = matches!(pass, MidiPass::Loop { marked: false, .. });
    // The last tick a note can start at and still be played: a key pressed within the last
    // samples of the range may round to the tick of its end, where the clip no longer plays.
    let last_start = end_tick.saturating_sub(1).max(0);
    let mut seq = Sequence::default();
    // The round of the loop being played: it goes up at every `WRAP`.
    let mut round = 0u32;
    // Keys down: (device, channel, pitch) -> (start tick, velocity, round, the sample the key was
    // pressed at). Whether the loop went round between a press and its release is judged by
    // their samples: two positions a few samples apart may share a tick, and a press in the
    // last samples of the range starts its note a tick earlier so that it plays.
    let mut down: HashMap<(u8, u8, u8), (i64, u8, u32, Samples)> = HashMap::new();
    let close = |seq: &mut Sequence,
                 (_, channel, pitch): (u8, u8, u8),
                 (start, velocity, since, pressed): (i64, u8, u32, Samples),
                 end: i64,
                 release: u8,
                 round: u32,
                 end_at: Samples| {
        let note = |start: i64, end: i64, release: u8| Note {
            pitch,
            velocity,
            release_velocity: release,
            channel,
            start,
            length: end.saturating_sub(start).max(1),
        };
        let rounds = round.saturating_sub(since);
        // A pass in a line never goes round, and neither does a loop pass between two
        // entries of one round when its rounds are marked: a release stamped before its
        // press ends the note at the press (a note is at least a tick long).
        if !looped || (rounds == 0 && (end_at >= pressed || !inferred)) {
            seq.notes.push(note(start, end, release));
            return;
        }
        // The pass went round between the key press and the release (loop recording goes back to
        // the start of the loop): the capture said so, or the release is earlier on the timeline
        // than the press. The note is held to the end of the pass and goes on from its start, as
        // it was played. Held for a whole round or more, it covers the loop: the second note
        // then ends where the first begins, so the two do not overlap.
        seq.notes.push(note(start, end_tick, 64));
        let end = if rounds > 1 || (rounds == 1 && end_at >= pressed) { start } else { end };
        if end > 0 {
            seq.notes.push(note(0, end, release));
        }
    };
    for e in events.iter().take(MAX_MIDI_TAKE_EVENTS) {
        // The mark sits at the end of the loop, which is the end of the range.
        if e.status == MidiTakeEvent::WRAP {
            round = round.saturating_add(1);
            continue;
        }
        if e.at < range.start || e.at >= range.end {
            continue;
        }
        let at = tick(e.at);
        if e.status == MidiTakeEvent::LOST {
            let ended: Vec<(u8, u8, u8)> = down.keys().copied().filter(|k| lost_covers(e.device, k.0)).collect();
            for key in ended {
                if let Some(open) = down.remove(&key) {
                    close(&mut seq, key, open, at, 64, round, e.at);
                }
            }
            continue;
        }
        let (channel, d1, d2) = (e.status & 0x0F, e.data1 & 0x7F, e.data2 & 0x7F);
        if midi_filtered(s, e.status) || !input.accepts(devices.get(usize::from(e.device)).map(String::as_str), channel) {
            continue;
        }
        let key = (e.device, channel, d1);
        // Like a note start, a controller in the last samples of the range stays where it plays.
        let ctrl = |kind, value| CtrlEvent { tick: at.min(last_start), channel, kind, value };
        match e.status & 0xF0 {
            0x90 if d2 > 0 => {
                // The same key again without a release: the first note ends here.
                if let Some(open) = down.insert(key, (at.min(last_start), d2, round, e.at)) {
                    close(&mut seq, key, open, at, 64, round, e.at);
                }
            }
            0x80 | 0x90 => {
                if let Some(open) = down.remove(&key) {
                    close(&mut seq, key, open, at, if e.status & 0xF0 == 0x80 { d2 } else { 64 }, round, e.at);
                }
            }
            0xA0 => seq.ctrls.push(ctrl(CtrlKind::PolyPressure(d1), i32::from(d2))),
            0xB0 => seq.ctrls.push(ctrl(CtrlKind::Cc(d1), i32::from(d2))),
            0xC0 => seq.ctrls.push(ctrl(CtrlKind::Program, i32::from(d1))),
            0xD0 => seq.ctrls.push(ctrl(CtrlKind::ChannelPressure, i32::from(d1))),
            0xE0 => seq.ctrls.push(ctrl(CtrlKind::PitchBend, (i32::from(d2) << 7 | i32::from(d1)) - 8192)),
            _ => {}
        }
    }
    // Still down when the pass stopped. A loop pass stops somewhere inside the loop: the keys end
    // there, in the round the stop is in. Otherwise, or when nothing says where it stopped, they
    // are held to the end of the range, in the round that was playing (after a round or more,
    // that is the whole loop).
    // A take cut at the event cap may have lost the marks of its last rounds: the stop position
    // belongs to a round the entries that are left do not reach, and is not used.
    let stop_at = match pass {
        MidiPass::Loop { stopped: Some(at), .. } if events.len() <= MAX_MIDI_TAKE_EVENTS => Some(at.clamp(range.start, range.end)),
        _ => None,
    };
    for (key, open) in down {
        close(&mut seq, key, open, stop_at.map_or(end_tick, tick), 64, round, stop_at.unwrap_or(range.end));
    }
    seq.sort();
    seq
}

/// Most events of one MIDI record pass that are kept.
pub const MAX_MIDI_TAKE_EVENTS: usize = 1 << 20;

/// Turn the MIDI played during a record pass that went once over `range` into a clip on every
/// MIDI and instrument track that is record-enabled now, each taking the events of its MIDI input
/// (device and channel). `devices` names the devices by [`MidiTakeEvent::device`]. A track that
/// received nothing gets no clip. One undo step ("Record"); with `same_pass` the clips join the
/// step the audio takes of this pass already made. The result says where input was dropped
/// during the pass.
pub fn add_midi_recording(e: &mut Engine, range: Range, events: &[MidiTakeEvent], devices: &[String], same_pass: bool) -> MidiTake {
    let inputs = midi_record_inputs(e);
    add_midi_recording_from(e, range, MidiPass::Linear, events, devices, &inputs, same_pass)
}

/// The MIDI inputs of the record-enabled MIDI and instrument tracks, to keep from the start of a
/// record pass and hand to [`add_midi_recording_from`] at its end.
pub fn midi_record_inputs(e: &Engine) -> Vec<(TrackId, Route)> {
    e.session().tracks.iter().filter(|t| records_midi_on(t)).map(|t| (t.id, t.mixer.input.clone())).collect()
}

/// [`add_midi_recording`] for the tracks that were in record when the pass started, with the MIDI
/// inputs they had then (`inputs`, from [`midi_record_inputs`]), and for a pass that went over
/// `range` as `pass` says. What a pass records is settled when it starts: a track listed takes
/// the events of that input for the whole pass, so changing its device or channel while recording
/// does not throw away what it already recorded, and neither does taking it out of record. A
/// track that is not listed gets nothing, also when it was record-enabled during the pass: it
/// would get what was played before that, and the caller that waits for the end of the take
/// decides whether to wait by this same list.
pub fn add_midi_recording_from(
    e: &mut Engine,
    range: Range,
    pass: MidiPass,
    events: &[MidiTakeEvent],
    devices: &[String],
    inputs: &[(TrackId, Route)],
    same_pass: bool,
) -> MidiTake {
    let before = e.session_arc();
    let before_takes = Arc::clone(&e.follow_takes);
    let mut out = MidiTake::default();
    if range.len() <= 0 {
        return out;
    }
    out.gaps =
        events.iter().take(MAX_MIDI_TAKE_EVENTS).filter(|ev| ev.status == MidiTakeEvent::LOST && ev.at >= range.start && ev.at < range.end).count()
            + usize::from(events.len() > MAX_MIDI_TAKE_EVENTS);
    let takes: Vec<_> = {
        let s = e.session();
        s.tracks
            .iter()
            .filter_map(|t| {
                let (_, route) = inputs.iter().find(|(id, _)| *id == t.id)?;
                let input = MidiInput::of(route);
                let score = midi_take_sequence(s, &input, range, pass, events, devices);
                let live = e
                    .playback_tempo
                    .as_ref()
                    .filter(|grid| *grid != &s.tempo)
                    .map(|grid| midi_take_on_grid(s, grid, &input, range, pass, events, devices));
                Some((t.id, t.name.clone(), score, live))
            })
            .filter(|(_, _, seq, _)| !seq.notes.is_empty() || !seq.ctrls.is_empty())
            .collect()
    };
    e.prune_clock_takes();
    for (tid, name, seq, live) in takes {
        let cid = e.session_mut().new_clip_id();
        if let (Some(live), Some(grid)) = (live, &e.playback_tempo) {
            let s = e.session();
            let projection = ClockTakeProjection::recorded(seq.clone(), live, s, grid, range);
            Arc::make_mut(&mut e.follow_takes).insert(cid, crate::ClockTake { capture: Arc::new(projection) });
        }
        crate::edit::place_clip(e, tid, Clip::midi(cid, name, range.start, range.len(), seq));
        out.clips.push(cid);
    }
    if !out.clips.is_empty() && !same_pass {
        e.push_undo_with_takes("Record", before, before_takes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_model::ClipContent;

    #[test]
    fn external_clock_take_uses_transient_grid_and_keeps_the_document_tempo() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Follow", "MIDI:*:keys");
        let original = e.session().tempo.clone();
        e.playback_tempo = Some(soundcraft_time::TempoMap::new(90., 4, 4).unwrap());
        let q = e.session().sample_rate.samples(60. / 90.);
        let range = Range::new(0, 3 * q);
        add_midi_recording(&mut e, range, &[ev(q, 0, 0x90, 65, 127), ev(2 * q, 0, 0x80, 65, 0)], &["keys".into()], false);
        let seq = sequence(&e, t, range);
        assert_eq!(seq.notes.len(), 1);
        assert_eq!(original.tick_to_samples(seq.notes[0].start, e.session().sample_rate), q);
        assert_eq!(e.session().tempo, original);
        let playback = e.playback_session();
        let ClipContent::Midi { sequence } = &playback.track(t).unwrap().clips()[0].content else { panic!("expected a MIDI clip") };
        assert_eq!(sequence.notes[0].start, soundcraft_time::TICKS_PER_QUARTER);
        assert_eq!(playback.tempo.tick_to_samples(sequence.notes[0].start, playback.sample_rate), q);
    }

    fn captured_clock_note() -> (Engine, TrackId, ClipId) {
        let mut e = Engine::default();
        let track = armed(&mut e, "Follow", "MIDI:*:keys");
        e.playback_tempo = Some(soundcraft_time::TempoMap::new(60., 4, 4).unwrap());
        let take =
            add_midi_recording(&mut e, Range::new(0, 96_000), &[ev(48_000, 0, 0x90, 65, 127), ev(72_000, 0, 0x80, 65, 0)], &["keys".into()], false);
        (e, track, take.clips[0])
    }

    #[test]
    fn clock_paste_merge_preserves_capture_and_ordinary_host_events_through_undo() {
        let mut e = Engine::default();
        let source = armed(&mut e, "Captured", "MIDI:*:keys");
        e.playback_tempo = Some(TempoMap::new(60., 4, 4).unwrap());
        let events = [ev(24_000, 0, 0xB0, 7, 101), ev(48_000, 0, 0x90, 65, 127), ev(72_000, 0, 0x80, 65, 0)];
        let take = add_midi_recording(&mut e, Range::new(0, 96_000), &events, &["keys".into()], false);
        let host_track = armed(&mut e, "Host", "MIDI:*:piano");
        let ClipContent::Midi { mut sequence } = e.session().find_clip(take.clips[0]).unwrap().1.content.clone() else { panic!("MIDI") };
        sequence.notes[0].pitch = 72;
        sequence.notes[0].start = 240;
        sequence.notes[0].length = 120;
        sequence.ctrls[0].tick = 360;
        sequence.ctrls[0].value = 31;
        let host = e.session_mut().new_clip_id();
        crate::edit::place_clip(&mut e, host_track, Clip::midi(host, "ordinary", 0, 192_000, sequence));
        e.execute("edit.copy", &json!({"tracks":[source.0],"start":0,"end":96_000})).unwrap();
        let merged = e.execute("edit.paste_merge_midi", &json!({"tracks":[host_track.0],"at":0})).unwrap();
        assert_eq!(merged["merged"], 1);
        let contents = |s: &Session| {
            let ClipContent::Midi { sequence } = &s.find_clip(host).unwrap().1.content else { panic!("MIDI") };
            (
                sequence.notes.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(),
                sequence.ctrls.iter().map(|c| (c.value, c.tick)).collect::<Vec<_>>(),
            )
        };
        assert_eq!(contents(&e.playback_session()), (vec![(72, 240), (65, 960)], vec![(31, 360), (101, 480)]));
        assert_eq!(contents(e.session()), (vec![(72, 240), (65, 1920)], vec![(31, 360), (101, 960)]));
        assert!(e.undo());
        assert_eq!(contents(&e.playback_session()), (vec![(72, 240)], vec![(31, 360)]));
        assert!(e.redo());
        assert_eq!(contents(&e.playback_session()), (vec![(72, 240), (65, 960)], vec![(31, 360), (101, 480)]));
        // Overwrite through both sides of the host. The left remnant keeps the host ID.
        // Undo must restore that ID's original per-event timing as well as its score.
        let overdub = add_midi_recording(
            &mut e,
            Range::new(6_000, 36_000),
            &[ev(12_000, 0, 0x90, 33, 100), ev(24_000, 0, 0x80, 33, 0)],
            &["piano".into()],
            false,
        );
        assert_eq!(overdub.clips.len(), 1);
        let after_overdub = e.playback_session();
        assert!(e.undo());
        assert_eq!(contents(&e.playback_session()), (vec![(72, 240), (65, 960)], vec![(31, 360), (101, 480)]));
        assert!(e.redo());
        assert_eq!(e.playback_session().track(host_track).unwrap().clips(), after_overdub.track(host_track).unwrap().clips());
        assert!(e.undo());
        let (bytes, _) = crate::io::session_bundle(e.session()).unwrap();
        let mut reopened = Engine::default();
        assert!(crate::io::open_session_bytes(&mut reopened, "Merged", &bytes).unwrap().is_empty());
        assert!(reopened.playback_tempo.is_none());
        assert_eq!(contents(&reopened.playback_session()), contents(e.session()));
    }

    #[test]
    fn clock_paste_merge_into_overlapping_hosts_preserves_their_visible_ends() {
        let (mut e, source, input_id) = captured_clock_note();
        e.session_mut().find_clip_mut(input_id).unwrap().length = 120_000;
        let target = armed(&mut e, "Host", "MIDI:*:piano");
        let ClipContent::Midi { mut sequence } = e.session().find_clip(input_id).unwrap().1.content.clone() else { panic!("expected MIDI") };
        sequence.notes[0].pitch = 72;
        sequence.notes[0].start = 1600;
        sequence.notes[0].length = 120;
        let host = e.session_mut().new_clip_id();
        let overlap = e.session_mut().new_clip_id();
        let mut locked = Clip::midi(overlap, "locked overlap", 72_000, 48_000, Sequence::default());
        locked.edit_locked = true;
        let playlist = e.session_mut().track_mut(target).unwrap().playlist_mut().unwrap();
        playlist.clips = vec![Clip::midi(host, "existing", 0, 96_000, sequence), locked];
        e.execute("edit.copy", &json!({"tracks":[source.0], "start":0,"end":120_000})).unwrap();
        e.execute("edit.paste_merge_midi", &json!({"tracks":[target.0],"at":0})).unwrap();
        let doc = e.playback_session();
        let c = doc.find_clip(host).unwrap().1;
        assert_eq!(c.end(), 96_000);
        let ClipContent::Midi { sequence } = &c.content else { panic!("expected MIDI") };
        assert!(sequence.notes.iter().any(|n| n.pitch == 72 && n.start == 1600));
        assert_eq!(doc.find_clip(overlap).unwrap().1.start, 72_000);
    }

    #[test]
    fn clock_merge_paste_to_empty_destination_keeps_copied_take_after_undo() {
        let (mut e, track, _) = captured_clock_note();
        e.execute("edit.copy", &json!({"tracks": [track.0], "start": 0, "end": 96_000})).unwrap();
        assert!(e.undo());
        e.execute("edit.paste_merge_midi", &json!({"tracks": [track.0], "at": 96_000})).unwrap();
        let doc = e.playback_session();
        let clip = &doc.track(track).unwrap().clips()[0];
        let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
        assert_eq!((clip.start, sequence.notes[0].start), (96_000, 960));
    }

    #[test]
    fn clock_pitch_split_preserves_both_parts_on_the_capture_grid() {
        let (mut e, track, clip) = captured_clock_note();
        let ClipContent::Midi { sequence } = &mut e.session_mut().find_clip_mut(clip).unwrap().content else { panic!("expected MIDI") };
        sequence.notes.push(soundcraft_midi::Note { pitch: 50, ..sequence.notes[0] });
        let split = e.execute("event.select_split_notes", &json!({"clips":[clip.0],"pitch_from":60,"action":"split"})).unwrap();
        let target = TrackId(split["tracks"][0].as_u64().unwrap());
        let doc = e.playback_session();
        for (track, pitch) in [(track, 50), (target, 65)] {
            let ClipContent::Midi { sequence } = &doc.track(track).unwrap().clips()[0].content else { panic!("expected MIDI") };
            assert_eq!((sequence.notes[0].pitch, sequence.notes[0].start, sequence.notes[0].length), (pitch, 960, 480));
        }
    }

    #[test]
    fn nested_consolidation_releases_previous_projections() {
        let (mut e, track, id) = captured_clock_note();
        let mut cid = id;
        let mut prior = vec![];
        for step in 1..=12 {
            let start = 96_000 + (step - 1) * 24_000;
            let id = e.session_mut().new_clip_id();
            crate::edit::place_clip(&mut e, track, Clip::midi(id, "append", start, 24_000, Sequence::default()));
            prior.push(Arc::downgrade(&e.follow_takes[&cid].capture));
            e.execute("edit.consolidate", &json!({"tracks":[track.0],"start":0,"end":start+24_000})).unwrap();
            cid = e.session().track(track).unwrap().clips()[0].id;
        }
        let last = e.follow_takes[&cid].clone();
        drop(e);
        assert!(prior.iter().all(|p| p.upgrade().is_none()), "the final take retained a previous projection");
        drop(last);
    }

    #[test]
    fn raw_anchor_velocity_and_cc_edits_keep_exact_recorded_ticks() {
        let mut e = Engine::default();
        let track = armed(&mut e, "Follow", "MIDI:*:keys");
        e.playback_tempo = Some(TempoMap::new(90., 4, 4).unwrap());
        let events =
            [ev(14, 0, 0x90, 65, 100), ev(14, 0, 0xB0, 7, 101), ev(47, 0, 0x80, 65, 20), ev(114, 0, 0x90, 70, 110), ev(147, 0, 0x80, 70, 10)];
        let take = add_midi_recording(&mut e, Range::new(0, 2_400), &events, &["keys".into()], false);
        let id = take.clips[0];
        let events = |s: &Session| {
            let ClipContent::Midi { sequence } = &s.track(track).unwrap().clips()[0].content else { panic!("MIDI") };
            (sequence.notes.clone(), sequence.ctrls.clone())
        };
        let (mut notes, mut ctrls) = events(&e.playback_session());
        assert_eq!((notes[0].start, notes[0].end(), ctrls[0].tick), (0, 1, 0));
        let score = events(e.session());
        assert_eq!((score.0[0].start, score.0[0].end(), score.1[0].tick), (1, 2, 1));
        e.execute("midi.note_edit", &json!({"clip": id.0, "index": 0, "velocity": 71})).unwrap();
        notes[0].velocity = 71;
        assert_eq!(events(&e.playback_session()), (notes.clone(), ctrls.clone()));
        let ClipContent::Midi { sequence } = &mut e.session_mut().find_clip_mut(id).unwrap().content else { panic!("MIDI") };
        sequence.ctrls[0].value = 57;
        ctrls[0].value = 57;
        assert_eq!(events(&e.playback_session()), (notes, ctrls));
    }

    fn identical_clock_events() -> (Engine, TrackId, ClipId) {
        let (mut e, track, id) = captured_clock_note();
        let ClipContent::Midi { sequence } = &mut e.session_mut().find_clip_mut(id).unwrap().content else { panic!("MIDI") };
        sequence.ctrls.push(CtrlEvent { tick: 1920, channel: 0, kind: CtrlKind::Cc(7), value: 90 });
        let mut sequence = sequence.clone();
        sequence.notes[0].start = 960;
        sequence.ctrls[0].tick = 960;
        let other = e.session_mut().new_clip_id();
        e.session_mut().track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(other, "overlap", 24_000, 120_000, sequence));
        e.execute("edit.consolidate", &json!({"tracks":[track.0],"start":0,"end":144_000})).unwrap();
        let clip = &e.session().track(track).unwrap().clips()[0];
        let id = clip.id;
        let ClipContent::Midi { sequence } = &clip.content else { panic!("MIDI") };
        assert_eq!(sequence.notes.len(), 2);
        assert_eq!(sequence.notes[0], sequence.notes[1]);
        assert_eq!(sequence.ctrls[0], sequence.ctrls[1]);
        (e, track, id)
    }

    #[test]
    fn deduplicate_anchor_keeps_first_equal_note_and_last_equal_controller() {
        let (mut e, track, id) = identical_clock_events();
        let ticks = |e: &Engine| {
            let doc = e.playback_session();
            let ClipContent::Midi { sequence } = &doc.track(track).unwrap().clips()[0].content else { panic!("MIDI") };
            (sequence.notes.iter().map(|n| n.start).collect::<Vec<_>>(), sequence.ctrls.iter().map(|c| c.tick).collect::<Vec<_>>())
        };
        assert_eq!(ticks(&e), (vec![960, 1440], vec![960, 1440]));
        let result = e.execute("event.remove_duplicate_notes", &json!({"clips":[id.0]})).unwrap();
        assert_eq!(result["removed"], 2);
        assert_eq!(ticks(&e), (vec![960], vec![1440]));
        assert!(e.undo());
        assert_eq!(ticks(&e), (vec![960, 1440], vec![960, 1440]));
    }

    #[test]
    fn indexed_anchor_delete_keeps_the_identical_survivors_attack() {
        let (mut e, track, id) = identical_clock_events();
        let attacks = |e: &Engine| {
            let doc = e.playback_session();
            let ClipContent::Midi { sequence } = &doc.track(track).unwrap().clips()[0].content else { panic!("MIDI") };
            sequence.notes.iter().map(|n| n.start).collect::<Vec<_>>()
        };
        assert_eq!(attacks(&e), [960, 1440]);
        for (deleted, survivor) in [(0, 1440), (1, 960)] {
            e.execute("midi.note_delete", &json!({"clip":id.0,"indices":[deleted,deleted,99]})).unwrap();
            assert_eq!(attacks(&e), [survivor]);
            assert!(e.undo());
            assert_eq!(attacks(&e), [960, 1440]);
            assert!(e.redo());
            assert_eq!(attacks(&e), [survivor]);
            assert!(e.undo());
        }
    }

    #[test]
    fn overlapping_consolidation_keeps_each_events_capture_after_velocity_edit() {
        let (mut e, track, id) = captured_clock_note();
        let ClipContent::Midi { sequence } = &mut e.session_mut().find_clip_mut(id).unwrap().content else { panic!("expected a MIDI clip") };
        sequence.ctrls.push(CtrlEvent { tick: 1920, channel: 0, kind: CtrlKind::Cc(7), value: 85 });
        let mut sequence = sequence.clone();
        sequence.notes[0].pitch = 72;
        sequence.notes[0].start = 960;
        sequence.ctrls[0].tick = 960;
        sequence.ctrls[0].kind = CtrlKind::Cc(10);
        let other = e.session_mut().new_clip_id();
        e.session_mut().track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(other, "overlap", 24_000, 72_000, sequence));
        e.execute("edit.consolidate", &json!({"tracks":[track.0],"start":0,"end":96_000})).unwrap();
        let cid = e.session().track(track).unwrap().clips()[0].id;
        let ClipContent::Midi { sequence } = &mut e.session_mut().find_clip_mut(cid).unwrap().content else { panic!("expected a MIDI clip") };
        sequence.notes[0].velocity = 78;
        sequence.ctrls[0].value = 12;
        let doc = e.playback_session();
        let ClipContent::Midi { sequence } = &doc.track(track).unwrap().clips()[0].content else { panic!("expected a MIDI clip") };
        assert_eq!(sequence.notes.iter().map(|n| (n.pitch, n.start, n.velocity)).collect::<Vec<_>>(), [(65, 960, 78), (72, 1440, 127)]);
        assert_eq!(
            sequence.ctrls.iter().map(|c| (c.kind, c.tick, c.value)).collect::<Vec<_>>(),
            [(CtrlKind::Cc(7), 960, 12), (CtrlKind::Cc(10), 1440, 85)]
        );
    }

    #[test]
    fn clock_consolidation_trims_each_grid_at_its_audible_range() {
        let (mut e, track, _) = captured_clock_note();
        e.execute("edit.copy", &json!({"tracks":[track.0],"start":0,"end":96_000})).unwrap();
        e.execute("edit.paste", &json!({"tracks":[track.0],"at":192_000})).unwrap();
        let ClipContent::Midi { mut sequence } = e.session().track(track).unwrap().clips()[0].content.clone() else { panic!("expected MIDI") };
        sequence.notes[0].pitch = 71;
        sequence.notes[0].start = 360;
        sequence.notes[0].length = 120;
        let id = e.session_mut().new_clip_id();
        crate::edit::place_clip(&mut e, track, Clip::midi(id, "score", 120_000, 48_000, sequence));
        e.execute("edit.consolidate", &json!({"tracks":[track.0],"start":132_000,"end":288_000})).unwrap();
        for (doc, expected) in [(e.session_arc(), vec![(71, 240), (65, 4320)]), (e.playback_session(), vec![(71, 120), (65, 2160)])] {
            let clip = doc.track(track).unwrap().clips().iter().find(|clip| clip.start == 132_000).unwrap();
            let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
            assert_eq!(sequence.notes.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), expected);
        }
        let clip = e.session().track(track).unwrap().clips().iter().find(|c| c.start == 132_000).unwrap().id;
        let ClipContent::Midi { sequence } = &mut e.session_mut().find_clip_mut(clip).unwrap().content else { panic!("expected a MIDI clip") };
        sequence.notes.iter_mut().find(|n| n.pitch == 65).unwrap().velocity = 81;
        let doc = e.playback_session();
        let ClipContent::Midi { sequence } = &doc.find_clip(clip).unwrap().1.content else { panic!("expected a MIDI clip") };
        assert_eq!(sequence.notes.iter().map(|n| (n.pitch, n.start, n.velocity)).collect::<Vec<_>>(), [(71, 120, 127), (65, 2160, 81)]);
    }

    #[test]
    fn clock_consolidation_keeps_notes_and_controllers_from_distinct_capture_grids() {
        let (mut e, track, _) = captured_clock_note();
        e.playback_tempo = Some(TempoMap::new(90., 4, 4).unwrap());
        let at = 168_000;
        add_midi_recording(
            &mut e,
            Range::new(120_000, 216_000),
            &[ev(at, 0, 0x90, 72, 100), ev(at, 0, 0xb0, 7, 100), ev(at + 12_000, 0, 0x80, 72, 0)],
            &["keys".into()],
            false,
        );
        e.playback_tempo = Some(TempoMap::new(60., 4, 4).unwrap());
        e.execute("edit.consolidate", &json!({"tracks":[track.0],"start":0,"end":216_000})).unwrap();
        let doc = e.playback_session();
        let ClipContent::Midi { sequence } = &doc.track(track).unwrap().clips()[0].content else { panic!("expected MIDI") };
        assert_eq!(sequence.notes.iter().map(|n| n.start).collect::<Vec<_>>(), [960, 3840]);
        assert_eq!(sequence.ctrls[0].tick, 3840);
    }

    #[test]
    fn clock_consolidation_preserves_normal_and_captured_timing_across_a_gap() {
        let (mut e, track, _) = captured_clock_note();
        let id = e.session_mut().new_clip_id();
        let ClipContent::Midi { mut sequence } = e.session().track(track).unwrap().clips()[0].content.clone() else { panic!("expected MIDI") };
        sequence.notes[0].pitch = 71;
        sequence.notes[0].start = 240;
        sequence.notes[0].length = 120;
        crate::edit::place_clip(&mut e, track, Clip::midi(id, "score", 120_000, 48_000, sequence));
        e.execute("edit.consolidate", &json!({"tracks":[track.0], "start":0,"end":168_000})).unwrap();
        for (doc, expected) in [(e.session_arc(), [1920, 5040]), (e.playback_session(), [960, 2640])] {
            let clip = &doc.track(track).unwrap().clips()[0];
            let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
            assert_eq!(sequence.notes.iter().map(|n| n.start).collect::<Vec<_>>(), expected);
        }
        // Subsequent edits cannot change which of the two source grids a note uses.
        let cid = e.session().track(track).unwrap().clips()[0].id;
        let ClipContent::Midi { sequence } = &mut e.session_mut().find_clip_mut(cid).unwrap().content else { panic!("expected MIDI") };
        for n in &mut sequence.notes {
            n.pitch += 1;
        }
        let doc = e.playback_session();
        let ClipContent::Midi { sequence } = &doc.track(track).unwrap().clips()[0].content else { panic!("expected MIDI") };
        assert_eq!(sequence.notes.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), [(66, 960), (72, 2640)]);
    }

    #[test]
    fn clock_regression_loop_copies_retain_the_recorded_attack() {
        let (mut e, track, clip) = captured_clock_note();
        e.execute("clip.loop", &json!({"clips": [clip.0], "count": 2})).unwrap();
        let playback = e.playback_session();
        let clips = playback.track(track).unwrap().clips();
        assert_eq!(clips.len(), 2);
        for clip in clips {
            let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
            assert_eq!(playback.tempo.tick_to_samples(sequence.notes[0].start, playback.sample_rate), 48_000);
        }
    }

    #[test]
    fn clock_regression_velocity_edit_keeps_the_recorded_attack() {
        let (mut e, _, clip) = captured_clock_note();
        e.execute("midi.note_edit", &json!({"clip": clip.0, "index": 0, "velocity": 72})).unwrap();
        let playback = e.playback_session();
        let ClipContent::Midi { sequence } = &playback.find_clip(clip).unwrap().1.content else { panic!("expected MIDI") };
        assert_eq!(sequence.notes[0].velocity, 72);
        assert_eq!(playback.tempo.tick_to_samples(sequence.notes[0].start, playback.sample_rate), 48_000);
    }

    #[test]
    fn clock_regression_copy_paste_keeps_the_take_grid_and_undo_restores_it() {
        let (mut e, t, _) = captured_clock_note();
        e.execute("edit.copy", &json!({"tracks": [t.0], "start": 0, "end": 96_000})).unwrap();
        e.execute("edit.paste", &json!({"tracks": [t.0], "at": 144_000})).unwrap();
        assert!(e.undo());
        assert!(e.redo());
        let playback = e.playback_session();
        let clips = playback.track(t).unwrap().clips();
        assert_eq!(clips.len(), 2);
        let ClipContent::Midi { sequence } = &clips[1].content else { panic!("expected MIDI") };
        assert_eq!(clips[1].start, 144_000);
        assert_eq!(playback.tempo.tick_to_samples(sequence.notes[0].start, playback.sample_rate), 48_000);
    }

    #[test]
    fn clock_regression_new_clip_after_undo_cannot_inherit_a_discarded_take() {
        let (mut e, t, original) = captured_clock_note();
        let ClipContent::Midi { sequence } = e.session().find_clip(original).unwrap().1.content.clone() else { panic!("expected MIDI") };
        assert!(e.undo());
        let s = e.session_mut();
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::midi(id, "authored", 0, 96_000, sequence));
        let playback = e.playback_session();
        let ClipContent::Midi { sequence } = &playback.find_clip(id).unwrap().1.content else { panic!("expected MIDI") };
        assert_eq!(sequence.notes[0].start, 2 * soundcraft_time::TICKS_PER_QUARTER);
    }

    #[test]
    fn clock_regression_edit_preserves_an_attack_in_the_final_sample() {
        let mut e = Engine::default();
        let track = armed(&mut e, "Follow", "MIDI:*:keys");
        e.playback_tempo = Some(TempoMap::new(60., 4, 4).unwrap());
        let range = Range::new(0, 96_000);
        let take = add_midi_recording(&mut e, range, &[ev(range.end - 1, 0, 0x90, 65, 127)], &["keys".into()], false);
        e.execute("midi.note_edit", &json!({"clip": take.clips[0].0, "index": 0, "velocity": 10})).unwrap();
        let playback = e.playback_session();
        let ClipContent::Midi { sequence } = &playback.track(track).unwrap().clips()[0].content else { panic!("expected MIDI") };
        assert_eq!(sequence.notes[0].start, 2 * soundcraft_time::TICKS_PER_QUARTER - 1);
        assert_eq!(sequence.notes[0].velocity, 10);
    }

    #[test]
    fn clock_regression_extract_midi_retains_the_take_grid() {
        let (mut engine, track, _) = captured_clock_note();
        let extracted = engine.execute("track.extract_midi", &json!({"tracks": [track.0]})).unwrap();
        let track = TrackId(extracted["tracks"][0]["track"].as_u64().unwrap());
        let playback = engine.playback_session();
        let clip = &playback.track(track).unwrap().clips()[0];
        let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
        assert_eq!(playback.tempo.tick_to_samples(sequence.notes[0].start, playback.sample_rate), 48_000);
    }

    #[test]
    fn clock_regression_duplicated_tracks_and_playlists_retain_the_captured_grid() {
        let (mut e, track, _) = captured_clock_note();
        e.execute("track.playlist_duplicate", &json!({"tracks": [track.0]})).unwrap();
        let copy = e.execute("track.duplicate", &json!({"tracks": [track.0]})).unwrap()["tracks"][0].as_u64().unwrap();
        let playback = e.playback_session();
        for id in [track, TrackId(copy)] {
            let playlists = &playback.track(id).unwrap().playlists;
            assert_eq!(playlists.len(), 2);
            for p in playlists {
                let ClipContent::Midi { sequence } = &p.clips[0].content else { panic!("expected MIDI") };
                assert_eq!(playback.tempo.tick_to_samples(sequence.notes[0].start, playback.sample_rate), 48_000);
            }
        }
    }

    #[test]
    fn clock_split_rebases_the_right_piece_at_its_capture_grid_origin() {
        let (mut e, track) = clock_take_across_tempo_change();
        e.execute("edit.separate", &json!({"tracks": [track.0], "at": 24_000})).unwrap();
        let playback = e.playback_session();
        let clips = playback.track(track).unwrap().clips();
        assert_eq!(clips.len(), 2);
        let ClipContent::Midi { sequence } = &clips[1].content else { panic!("expected MIDI") };
        assert_eq!(sequence.notes[0].start, 480);
        assert_eq!(sequence.notes[0].length, 480);
    }

    #[test]
    fn clock_split_clipboard_take_keeps_its_grid_after_undoing_record() {
        let (mut e, track, _) = captured_clock_note();
        e.execute("edit.copy", &json!({"tracks": [track.0], "start": 0, "end": 96_000})).unwrap();
        assert!(e.undo());
        assert!(e.session().track(track).unwrap().clips().is_empty());
        e.execute("edit.paste", &json!({"tracks": [track.0], "at": 144_000})).unwrap();
        let playback = e.playback_session();
        let clip = &playback.track(track).unwrap().clips()[0];
        let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
        assert_eq!(clip.start, 144_000);
        assert_eq!(sequence.notes[0].start, 960);
    }

    #[test]
    fn clock_split_batched_and_implicit_edits_preserve_right_piece_ancestry() {
        for case in ["batch", "locked", "clear", "shuffle"] {
            let (mut e, track, id) = captured_clock_note();
            match case {
                "batch" => {
                    assert_eq!(crate::edit::separate_at_points(&mut e, track, &[24_000, 12_000, 24_000]), 2);
                }
                "locked" => {
                    e.session_mut().find_clip_mut(id).unwrap().time_locked = true;
                    e.execute("event.insert_time", &json!({"start": 24_000, "end": 36_000})).unwrap();
                }
                "clear" => {
                    e.execute("edit.clear", &json!({"tracks": [track.0], "start": 12_000, "end": 24_000})).unwrap();
                }
                _ => {
                    crate::edit::make_room(&mut e, track, 24_000, 12_000).unwrap();
                }
            }
            let playback = e.playback_session();
            let clip = playback.track(track).unwrap().clips().last().unwrap();
            let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
            assert_eq!(sequence.notes[0].start, 480, "{case}");
        }
    }

    #[test]
    fn clock_split_clipboard_does_not_follow_a_reused_document_id() {
        let (mut e, track, old) = captured_clock_note();
        e.execute("edit.copy", &json!({"tracks": [track.0], "start": 0, "end": 96_000})).unwrap();
        assert!(e.undo());
        e.playback_tempo = Some(TempoMap::new(90., 4, 4).unwrap());
        let replacement =
            add_midi_recording(&mut e, Range::new(0, 96_000), &[ev(48_000, 0, 0x90, 65, 127), ev(72_000, 0, 0x80, 65, 0)], &["keys".into()], false);
        assert_eq!(replacement.clips, [old]);
        e.execute("edit.paste", &json!({"tracks": [track.0], "at": 144_000})).unwrap();
        let playback = e.playback_session();
        let attacks: Vec<_> = playback
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .map(|clip| {
                let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
                sequence.notes[0].start
            })
            .collect();
        assert_eq!(attacks, [1440, 960]);
    }

    fn clock_take_across_tempo_change() -> (Engine, TrackId) {
        let mut e = Engine::default();
        let track = armed(&mut e, "Follow", "MIDI:*:keys");
        e.session_mut().tempo.set_tempo(960, 240.).unwrap();
        e.playback_tempo = Some(TempoMap::new(60., 4, 4).unwrap());
        add_midi_recording(
            &mut e,
            Range::new(0, 96_000),
            &[ev(30_000, 0, 0xB0, 7, 101), ev(48_000, 0, 0x90, 65, 127), ev(72_000, 0, 0x80, 65, 0)],
            &["keys".into()],
            false,
        );
        (e, track)
    }

    fn attack_samples(e: &Engine, track: TrackId, start: Samples) -> Samples {
        let playback = e.playback_session();
        let clip = playback.track(track).unwrap().clips().iter().find(|c| c.start == start).unwrap();
        let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
        let origin_tick = playback.tempo.samples_to_ticks(start, playback.sample_rate);
        playback.tempo.tick_to_samples(origin_tick + sequence.notes[0].start, playback.sample_rate)
    }

    #[test]
    fn clock_range_left_trim_and_undo_keep_the_moved_take_at_192000_samples() {
        for command in ["edit.trim_to_selection", "edit.trim_start_to_insertion", "edit.trim_clip"] {
            let (mut engine, track) = clock_take_across_tempo_change();
            engine.execute("edit.copy", &json!({"tracks": [track.0], "start": 0, "end": 96_000})).unwrap();
            engine.execute("edit.paste", &json!({"tracks": [track.0], "at": 144_000})).unwrap();
            let id = engine.session().track(track).unwrap().clips()[1].id;
            let params = json!({"tracks": [track.0], "clip": id.0, "at": 168_000, "start": 168_000, "end": 240_000});
            let assert_ctrl = |e: &Engine, tick| {
                let doc = e.playback_session();
                let ClipContent::Midi { sequence } = &doc.find_clip(id).unwrap().1.content else { panic!("expected MIDI") };
                assert_eq!(sequence.ctrls.iter().map(|c| (c.tick, c.value)).collect::<Vec<_>>(), [(tick, 101)], "{command}");
            };
            engine.execute(command, &params).unwrap();
            assert_ctrl(&engine, 120);
            assert_eq!(attack_samples(&engine, track, 168_000), 192_000, "{command}");
            assert!(engine.undo());
            assert_ctrl(&engine, 600);
            assert_eq!(attack_samples(&engine, track, 144_000), 192_000, "Undo {command}");
            assert!(engine.redo());
            assert_ctrl(&engine, 120);
            assert_eq!(attack_samples(&engine, track, 168_000), 192_000, "Redo {command}");
        }
    }

    #[test]
    fn clock_rendered_range_owns_attacks_and_controllers_at_their_audible_samples() {
        for bpm in [60., 90., 45.] {
            for op in ["split", "trim", "copy"] {
                let mut engine = Engine::default();
                let track = armed(&mut engine, "Follow", "MIDI:*:keys");
                engine.playback_tempo = Some(TempoMap::new(60., 4, 4).unwrap());
                add_midi_recording(
                    &mut engine,
                    Range::new(0, 96_000),
                    &[
                        ev(48_000, 0, 0x90, 65, 111),
                        ev(51_000, 0, 0x80, 65, 0),
                        ev(54_000, 0, 0xB0, 7, 99),
                        ev(60_000, 0, 0x90, 67, 97),
                        ev(66_000, 0, 0x80, 67, 0),
                        ev(72_000, 0, 0xB0, 10, 71),
                    ],
                    &["keys".into()],
                    false,
                );
                engine.playback_tempo = Some(TempoMap::new(bpm, 4, 4).unwrap());
                let events = |e: &Engine, from, to, subtract| {
                    let doc = e.playback_session();
                    let mut result = Vec::new();
                    for clip in doc.track(track).unwrap().clips().iter().filter(|c| (from..to).contains(&c.start)) {
                        let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
                        let base = doc.tempo.samples_to_ticks(clip.start, doc.sample_rate);
                        let sample = |tick| doc.tempo.tick_to_samples(base + tick, doc.sample_rate) - subtract;
                        for n in sequence.notes.iter().filter(|n| clip.range().contains(sample(n.start) + subtract)) {
                            result.push((n.pitch, sample(n.start), sample(n.end())));
                        }
                        for c in sequence.ctrls.iter().filter(|c| clip.range().contains(sample(c.tick) + subtract)) {
                            result.push((c.value as u8, sample(c.tick), sample(c.tick)));
                        }
                    }
                    result.sort_unstable();
                    result
                };
                let split = 36_000;
                let before = events(&engine, 0, 1, 0);
                let expected = |lo, hi| before.iter().copied().filter(|(_, at, _)| (lo..hi).contains(at)).collect::<Vec<_>>();
                if bpm == 90. {
                    assert!(before.contains(&(65, 32_000, 34_000)));
                    assert!(before.contains(&(99, split, split)));
                }
                match op {
                    "split" => {
                        engine.execute("edit.separate", &json!({"tracks":[track.0],"at":split})).unwrap();
                        assert_eq!(events(&engine, 0, 1, 0), expected(0, split), "left {bpm}");
                        assert_eq!(events(&engine, split, split + 1, 0), expected(split, 96_000), "right {bpm}");
                    }
                    "trim" => {
                        engine.execute("edit.trim_start_to_insertion", &json!({"tracks":[track.0],"at":split})).unwrap();
                        assert_eq!(events(&engine, split, split + 1, 0), expected(split, 96_000), "trim {bpm}");
                    }
                    _ => {
                        engine.execute("edit.copy", &json!({"tracks":[track.0],"start":split,"end":84_000})).unwrap();
                        engine.execute("edit.paste", &json!({"tracks":[track.0],"at":144_000})).unwrap();
                        assert_eq!(events(&engine, 144_000, 144_001, 144_000 - split), expected(split, 84_000), "copy {bpm}");
                    }
                }
            }
        }
    }

    #[test]
    fn clock_split_keeps_score_events_temporarily_outside_the_source() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Follow", "MIDI:*:keys");
        e.playback_tempo = Some(TempoMap::new(60., 4, 4).unwrap());
        add_midi_recording(
            &mut e,
            Range::new(0, 96_000),
            &[ev(84_000, 0, 0x90, 65, 127), ev(88_000, 0, 0x80, 65, 0), ev(84_000, 0, 0xb0, 7, 88)],
            &["keys".into()],
            false,
        );
        e.playback_tempo = Some(TempoMap::new(45., 4, 4).unwrap());
        e.execute("edit.separate", &json!({"tracks":[t.0],"at":36_000})).unwrap();
        let score = e.session();
        let clips = score.track(t).unwrap().clips();
        let ClipContent::Midi { sequence: left } = &clips[0].content else { panic!("MIDI") };
        let ClipContent::Midi { sequence: right } = &clips[1].content else { panic!("MIDI") };
        assert!(left.notes.is_empty() && left.ctrls.is_empty());
        assert_eq!((right.notes.len(), right.ctrls.len()), (1, 1), "keep the editable attack and CC");
        assert_eq!(attack_and_cc_samples(score, t), [(65, 84_000), (88, 84_000)]);
        assert!(attack_and_cc_samples(&e.playback_session(), t).is_empty(), "the slow clock still places both events outside the clip");
        e.playback_tempo = Some(TempoMap::new(60., 4, 4).unwrap());
        assert_eq!(attack_and_cc_samples(&e.playback_session(), t).len(), 2, "both events sound again on return to capture tempo");
    }

    /// Count only attacks and CCs that fall inside their own clip, like the audio scheduler.
    fn attack_and_cc_samples(doc: &Session, t: TrackId) -> Vec<(i32, Samples)> {
        let mut events = Vec::new();
        for clip in doc.track(t).unwrap().clips() {
            let ClipContent::Midi { sequence } = &clip.content else { continue };
            let origin = doc.tempo.samples_to_ticks(clip.start, doc.sample_rate);
            for (value, tick) in sequence.notes.iter().map(|n| (i32::from(n.pitch), n.start)).chain(sequence.ctrls.iter().map(|c| (c.value, c.tick)))
            {
                let at = doc.tempo.tick_to_samples(origin + tick, doc.sample_rate);
                if clip.range().contains(at) {
                    events.push((value, at));
                }
            }
        }
        events.sort_unstable();
        events
    }

    #[test]
    fn clock_consolidation_checks_half_open_samples_for_notes_and_ccs() {
        for mixed in [false, true] {
            for end in [48_000, 48_001] {
                let mut e = Engine::default();
                let t = armed(&mut e, "Follow", "MIDI:*:keys");
                e.playback_tempo = Some(TempoMap::new(60., 4, 4).unwrap());
                let take = add_midi_recording(
                    &mut e,
                    Range::new(0, 96_000),
                    &[ev(48_000, 0, 0x90, 65, 127), ev(72_000, 0, 0x80, 65, 0), ev(48_000, 0, 0xb0, 7, 88)],
                    &["keys".into()],
                    false,
                );
                let mut expected = vec![(65, 48_000), (88, 48_000)];
                if mixed {
                    let ClipContent::Midi { mut sequence } = e.session().find_clip(take.clips[0]).unwrap().1.content.clone() else { panic!("MIDI") };
                    sequence.notes[0].pitch = 70;
                    sequence.notes[0].start = 960;
                    sequence.ctrls[0].tick = 960;
                    sequence.ctrls[0].value = 99;
                    let doc = e.session_mut();
                    let id = doc.new_clip_id();
                    doc.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(id, "ordinary", 0, 96_000, sequence));
                    expected.extend([(70, 48_000), (99, 48_000)]);
                    expected.sort_unstable();
                }
                assert_eq!(attack_and_cc_samples(&e.playback_session(), t), expected);
                e.execute("edit.consolidate", &json!({"tracks":[t.0],"start":0,"end":end})).unwrap();
                let playback = e.playback_session();
                let clip = playback.track(t).unwrap().clips().iter().find(|c| c.start == 0).unwrap();
                let ClipContent::Midi { sequence } = &clip.content else { panic!("MIDI") };
                let pairs = if end == 48_000 { 0 } else { 1 + usize::from(mixed) };
                assert_eq!((sequence.notes.len(), sequence.ctrls.len()), (pairs, pairs), "end {end}, mixed {mixed}");
                assert_eq!(attack_and_cc_samples(&playback, t), expected, "exactly once across the consolidated and remaining material");
                let saved = attack_and_cc_samples(e.session(), t).into_iter().map(|(event, _)| event).collect::<Vec<_>>();
                assert_eq!(saved, expected.iter().map(|&(event, _)| event).collect::<Vec<_>>(), "one editable copy per source event");
            }
        }
    }

    #[test]
    fn clock_rendered_range_cut_one_sample_from_an_attack_keeps_its_exact_tick() {
        for cut in [31_999, 32_000, 32_001] {
            let (mut engine, track, _) = captured_clock_note();
            engine.playback_tempo = Some(TempoMap::new(90., 4, 4).unwrap());
            engine.execute("edit.separate", &json!({"tracks":[track.0],"at":cut})).unwrap();
            let owner = if cut <= 32_000 { cut } else { 0 };
            assert_eq!(attack_samples(&engine, track, owner), 32_000);
            let playback = engine.playback_session();
            let total: usize = playback
                .track(track)
                .unwrap()
                .clips()
                .iter()
                .map(|c| {
                    let ClipContent::Midi { sequence } = &c.content else { panic!("expected MIDI") };
                    sequence.notes.len()
                })
                .sum();
            assert_eq!(total, 1);
        }
    }

    #[test]
    fn clock_range_split_after_move_keeps_the_attack_at_192000_samples() {
        let (mut e, t) = clock_take_across_tempo_change();
        e.execute("edit.copy", &json!({"tracks": [t.0], "start": 0, "end": 96_000})).unwrap();
        e.execute("edit.paste", &json!({"tracks": [t.0], "at": 144_000})).unwrap();
        assert_eq!(attack_samples(&e, t, 144_000), 192_000);
        e.execute("edit.separate", &json!({"tracks": [t.0], "at": 168_000})).unwrap();
        assert_eq!(attack_samples(&e, t, 168_000), 192_000);
        // The controller at original sample 30,000 falls between the correct cut and the
        // destination-tempo cut. It must stay on the right at its original relative time.
        let playback = e.playback_session();
        let right = playback.track(t).unwrap().clips().last().unwrap();
        let ClipContent::Midi { sequence } = &right.content else { panic!("expected MIDI") };
        assert_eq!(sequence.ctrls.len(), 1);
        assert_eq!((sequence.ctrls[0].tick, sequence.ctrls[0].value), (120, 101));
    }

    #[test]
    fn clock_range_partial_duplicates_use_the_trimmed_capture_origin() {
        for cmd in ["edit.duplicate", "edit.repeat", "edit.duplicate_extend"] {
            let (mut e, t) = clock_take_across_tempo_change();
            e.execute(cmd, &json!({"tracks": [t.0], "start": 24_000, "end": 72_000, "count": 1})).unwrap();
            assert_eq!(attack_samples(&e, t, 72_000), 96_000, "{cmd}");
        }
    }

    #[test]
    fn clock_range_playlist_placements_retain_whole_and_trimmed_capture_origins() {
        for cmd in ["edit.copy_to_duplicate_playlist", "edit.move_to_new_playlist", "edit.copy_to_new_playlist"] {
            let (mut e, t) = clock_take_across_tempo_change();
            e.execute(cmd, &json!({"tracks": [t.0], "start": 24_000, "end": 72_000})).unwrap();
            e.execute("track.playlist_select", &json!({"tracks": [t.0], "index": 1})).unwrap();
            assert_eq!(attack_samples(&e, t, 24_000), 48_000, "{cmd}");
        }
    }

    #[test]
    fn clock_range_playlist_promotion_and_unselected_duplicate_keep_their_own_origins() {
        let (mut e, t) = clock_take_across_tempo_change();
        let sr = e.session().sample_rate;
        let make_copy = json!({"tracks": [t.0], "start": 0, "end": 96_000});
        e.execute("edit.copy", &make_copy).unwrap();
        e.execute("edit.paste", &json!({"tracks": [t.0], "at": 144_000})).unwrap();
        e.execute("edit.copy_to_duplicate_playlist", &json!({"tracks": [t.0], "start": 24_000, "end": 72_000})).unwrap();
        e.execute("track.playlist_select", &json!({"tracks": [t.0], "index": 1})).unwrap();
        assert_eq!(attack_samples(&e, t, 144_000), 192_000, "whole unselected clip");
        e.execute("track.playlist_new", &json!({"tracks": [t.0]})).unwrap();
        // This selection starts halfway through the relocated copy's first second. Its
        // original tempo change is still 24,000 samples after the start of that copy.
        e.execute("track.playlist_promote", &json!({"track": t.0, "playlist": 1, "start": 168_000, "end": 216_000})).unwrap();
        assert_eq!(attack_samples(&e, t, 168_000), 192_000);
        let playback = e.playback_session();
        let clip = &playback.track(t).unwrap().clips()[0];
        let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI") };
        assert_eq!(playback.tempo.tick_to_samples(sequence.notes[0].length, sr), 24_000);
    }

    fn armed(e: &mut Engine, name: &str, input: &str) -> TrackId {
        let id = e.execute("track.new", &json!({"name": name, "kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [id], "input": input})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
        TrackId(id)
    }

    fn ev(at: Samples, device: u8, status: u8, data1: u8, data2: u8) -> MidiTakeEvent {
        MidiTakeEvent { at, device, status, data1, data2 }
    }

    fn sequence(e: &Engine, t: TrackId, range: Range) -> Sequence {
        let clip = e.session().track(t).unwrap().clips().first().unwrap().clone();
        assert_eq!(clip.range(), range);
        let ClipContent::Midi { sequence } = clip.content else { panic!("not a MIDI clip") };
        sequence
    }

    #[test]
    fn midi_played_during_a_pass_becomes_a_clip_on_each_record_enabled_midi_track() {
        let mut e = Engine::default();
        let all = armed(&mut e, "Keys", "none");
        let pad = armed(&mut e, "Pad", "MIDI:10:Pad");
        let idle = armed(&mut e, "Idle", "MIDI:*:Nothing");
        assert_eq!(e.session().busses.len(), 0, "a MIDI input is not a bus name");
        assert!(records_midi(&e));
        let events = [
            ev(40_000, 0, 0x90, 59, 80),  // before the pass: left out, and so is its release
            ev(48_000, 0, 0x90, 60, 100), // one beat at 120 bpm, 48 kHz: 24 000 samples = 960 ticks
            ev(50_000, 0, 0x80, 59, 0),
            ev(60_000, 1, 0x99, 36, 127),
            ev(72_000, 0, 0x80, 60, 30),
            ev(72_000, 0, 0xE0, 0, 96),
            ev(84_000, 1, 0x99, 36, 0),
            ev(90_000, 0, 0xB0, 64, 127),
            ev(95_000, 0, 0x90, 67, 90), // still down at the end
            ev(96_000, 0, 0x90, 72, 90), // after the pass
        ];
        let devices = ["Keys".to_string(), "Pad".to_string()];
        let range = Range::new(48_000, 96_000);
        let before = e.session().tracks.iter().map(|t| t.clips().len()).sum::<usize>();
        let take = add_midi_recording(&mut e, range, &events, &devices, false);
        assert_eq!((before, take.clips.len(), take.gaps), (0, 2, 0), "the track whose device was silent gets no clip");
        let keys = sequence(&e, all, range);
        let notes: Vec<(u8, u8, i64, i64, u8)> = keys.notes.iter().map(|n| (n.channel, n.pitch, n.start, n.length, n.release_velocity)).collect();
        assert_eq!(notes, [(0, 60, 0, 960, 30), (9, 36, 480, 960, 64), (0, 67, 1880, 40, 64)]);
        assert_eq!(
            keys.ctrls.iter().map(|c| (c.tick, c.kind, c.value)).collect::<Vec<_>>(),
            [(960, CtrlKind::PitchBend, 4096), (1680, CtrlKind::Cc(64), 127)]
        );
        let pads = sequence(&e, pad, range);
        assert_eq!((pads.notes.len(), pads.notes[0].pitch, pads.ctrls.len()), (1, 36, 0), "only its device and channel");
        assert!(e.session().track(idle).unwrap().clips().is_empty());
        // One undo step for the pass; the input filter keeps kinds out.
        e.run("edit.undo").unwrap();
        assert_eq!(e.session().tracks.iter().map(|t| t.clips().len()).sum::<usize>(), 0);
        e.execute("setup.midi_input_filter", &json!({"filter": ["pitch_bend", "cc"]})).unwrap();
        add_midi_recording(&mut e, range, &events, &devices, false);
        assert!(sequence(&e, all, range).ctrls.is_empty());
        assert!(add_midi_recording(&mut e, Range::new(5, 5), &events, &devices, false).clips.is_empty());
    }

    #[test]
    fn the_same_pitch_on_two_devices_is_two_notes_and_lost_input_ends_only_its_device() {
        let mut e = Engine::default();
        let all = armed(&mut e, "Keys", "none");
        let range = Range::new(0, 96_000);
        let events = [
            ev(0, 0, 0x90, 60, 100),
            ev(12_000, 1, 0x90, 60, 90), // the same pitch and channel from another device
            ev(24_000, 0, 0x80, 60, 0),  // ends the first device's note only
            ev(24_000, 0, 0x90, 64, 100),
            ev(36_000, 0, MidiTakeEvent::LOST, 0, 0), // device 0 lost input: its note-off never comes
            ev(48_000, 1, 0x80, 60, 0),
            ev(60_000, 1, 0x90, 67, 100),
            ev(72_000, u8::MAX, MidiTakeEvent::LOST, 0, 0), // every device
            ev(200_000, 0, MidiTakeEvent::LOST, 0, 0),      // after the pass
        ];
        let take = add_midi_recording(&mut e, range, &events, &["A".to_string(), "B".to_string()], false);
        assert_eq!((take.clips.len(), take.gaps), (1, 2), "the take says it is incomplete");
        let notes: Vec<(u8, i64, i64)> = sequence(&e, all, range).notes.iter().map(|n| (n.pitch, n.start, n.length)).collect();
        assert_eq!(notes, [(60, 0, 960), (60, 480, 1440), (64, 960, 480), (67, 2400, 480)]);
    }
    /// `(channel, pitch, start, length, velocity, release velocity)` of every note.
    fn notes_of(seq: &Sequence) -> Vec<(u8, u8, i64, i64, u8, u8)> {
        seq.notes.iter().map(|n| (n.channel, n.pitch, n.start, n.length, n.velocity, n.release_velocity)).collect()
    }

    #[test]
    fn note_edge_cases_make_the_notes_a_player_would_expect() {
        // 120 bpm at 48 kHz: a quarter note is 24 000 samples and 960 ticks.
        let s = Session::default();
        let range = Range::new(0, 96_000);
        let q = 24_000;
        let cases: [(&str, Vec<MidiTakeEvent>, Vec<(u8, u8, i64, i64, u8, u8)>); 7] = [
            (
                "the same key again without a release: the first note ends where the second starts",
                vec![ev(0, 0, 0x90, 60, 100), ev(q, 0, 0x90, 60, 90), ev(2 * q, 0, 0x80, 60, 0)],
                vec![(0, 60, 0, 960, 100, 64), (0, 60, 960, 960, 90, 0)],
            ),
            ("a note-on with velocity 0 is a release", vec![ev(0, 0, 0x90, 60, 100), ev(q, 0, 0x90, 60, 0)], vec![(0, 60, 0, 960, 100, 64)]),
            ("a note-off keeps its release velocity", vec![ev(0, 0, 0x90, 60, 100), ev(q, 0, 0x80, 60, 30)], vec![(0, 60, 0, 960, 100, 30)]),
            (
                "data bytes above 127 are masked",
                vec![ev(0, 0, 0x91, 0x80 | 60, 0x80 | 100), ev(q, 0, 0x81, 0x80 | 60, 0xFF)],
                vec![(1, 60, 0, 960, 100, 127)],
            ),
            ("a key still down at the end of the pass ends there", vec![ev(q, 0, 0x90, 60, 100)], vec![(0, 60, 960, 2880, 100, 64)]),
            ("a release without a key down is nothing", vec![ev(q, 0, 0x80, 60, 0), ev(q, 0, 0x90, 61, 0)], vec![]),
            (
                "a note on and off at the same sample is one tick long, not empty",
                vec![ev(q, 0, 0x90, 60, 100), ev(q, 0, 0x80, 60, 0)],
                vec![(0, 60, 960, 1, 100, 0)],
            ),
        ];
        for (what, events, want) in cases {
            let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &events, &[]);
            assert_eq!(notes_of(&seq), want, "{what}");
            assert!(seq.notes.iter().all(|n| n.length > 0 && n.pitch < 128 && n.velocity < 128 && n.channel < 16), "{what}");
        }
        // Controller data is masked too: a pitch bend of two 0xFF bytes is full up, not more.
        let seq =
            midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &[ev(0, 0, 0xE0, 0xFF, 0xFF), ev(0, 0, 0xB0, 0x87, 0xFF)], &[]);
        assert_eq!(seq.ctrls.iter().map(|c| (c.kind, c.value)).collect::<Vec<_>>(), [(CtrlKind::PitchBend, 8191), (CtrlKind::Cc(7), 127)]);
    }

    #[test]
    fn a_note_held_across_the_loop_point_is_split_at_it() {
        // A loop of four quarter notes (3840 ticks), recorded over several times: the timeline
        // position goes back to the start, so a release can be earlier than its key press.
        let s = Session::default();
        let (q, range) = (24_000, Range::new(0, 96_000));
        let events = [
            ev(q / 2, 0, 0x90, 64, 100), // within one time round: an ordinary note
            ev(q, 0, 0x80, 64, 0),
            ev(3 * q + q / 2, 0, 0x90, 60, 100), // pressed half a quarter before the loop point…
            ev(q / 2, 0, 0x80, 60, 40),          // …released half a quarter after it
            ev(3 * q, 0, 0x90, 67, 100),         // pressed, then pressed again after the loop point
            ev(q, 0, 0x90, 67, 90),
            ev(2 * q, 0, 0x80, 67, 0),
            ev(3 * q, 1, 0x90, 72, 100), // its device loses input after the loop point
            ev(q, 1, MidiTakeEvent::LOST, 0, 0),
            ev(3 * q, 0, 0x90, 48, 100), // released exactly at the loop start: nothing after it
            ev(0, 0, 0x80, 48, 0),
        ];
        let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: false }, &events, &[]);
        let mut got = notes_of(&seq);
        got.sort_by_key(|n| (n.1, n.2));
        let want = [
            (0, 48, 2880, 960, 100, 64),
            (0, 60, 0, 480, 100, 40),    // from the loop start to the release
            (0, 60, 3360, 480, 100, 64), // from the key press to the loop point
            (0, 64, 480, 480, 100, 0),
            (0, 67, 0, 960, 100, 64), // the first press, carried over the loop point…
            (0, 67, 960, 960, 90, 0), // …ends where the key is pressed again
            (0, 67, 2880, 960, 100, 64),
            (0, 72, 0, 960, 100, 64),
            (0, 72, 2880, 960, 100, 64),
        ];
        assert_eq!(got, want);
        assert!(seq.notes.iter().all(|n| n.start >= 0 && n.start + n.length <= 3840), "every note is inside the loop");
    }

    #[test]
    fn a_note_held_into_the_next_round_is_split_even_when_its_release_is_later_on_the_timeline() {
        // The same loop. The capture says where it went round; positions alone cannot.
        let s = Session::default();
        let (q, range) = (24_000, Range::new(0, 96_000));
        let wrap = ev(4 * q, u8::MAX, MidiTakeEvent::WRAP, 0, 0);
        let events = [
            ev(q / 2, 0, 0x90, 60, 100), // pressed at half a quarter in the first round…
            ev(q, 0, 0x90, 64, 100),     // (an ordinary note within the first round)
            ev(2 * q, 0, 0x80, 64, 0),
            ev(3 * q, 0, 0x90, 67, 100), // (held over two rounds)
            wrap,
            ev(q, 0, 0x80, 60, 40),      // …released at one quarter in the second
            ev(2 * q, 0, 0x90, 72, 100), // an ordinary note within the second round
            ev(3 * q, 0, 0x80, 72, 0),
            wrap,
            ev(q, 0, 0x80, 67, 0),
            ev(2 * q, 0, 0x90, 48, 100), // still down when the pass ends: to the end of the loop
        ];
        let held = events;
        let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: true }, &events, &[]);
        let mut got = notes_of(&seq);
        got.sort_by_key(|n| (n.1, n.2));
        let want = [
            (0, 48, 1920, 1920, 100, 64),
            (0, 60, 0, 480, 100, 40),    // from the loop start: up to its own start, so the two
            (0, 60, 480, 3360, 100, 64), // parts cover what was held and do not overlap
            (0, 64, 960, 960, 100, 0),
            (0, 67, 0, 2880, 100, 0), // held for more than a round: the whole loop
            (0, 67, 2880, 960, 100, 64),
            (0, 72, 1920, 960, 100, 0),
        ];
        assert_eq!(got, want);
        assert!(seq.notes.iter().all(|n| n.start >= 0 && n.start + n.length <= 3840), "every note is inside the loop");
        // Keys still down when the pass stops, after the loop went round. One was pressed at half
        // a quarter in the first round, one at three quarters in the second.
        let keys = [ev(q / 2, 0, 0x90, 60, 100), wrap, ev(3 * q, 0, 0x90, 64, 100), wrap];
        let stopped = |at: i64| {
            let mut got =
                notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: Some(at), marked: true }, &keys, &[]));
            got.sort_by_key(|n| (n.1, n.2));
            got.iter().map(|n| (n.1, n.2, n.3)).collect::<Vec<_>>()
        };
        // Stopped at one quarter in the third round: the first key was held for two rounds and
        // covers the loop; the second went round once and is held from the loop start to the stop.
        assert_eq!(stopped(q), [(60, 0, 480), (60, 480, 3360), (64, 0, 960), (64, 2880, 960)]);
        // Stopped past both presses: both cover the loop, without overlapping themselves.
        assert_eq!(stopped(3 * q + q / 2), [(60, 0, 480), (60, 480, 3360), (64, 0, 2880), (64, 2880, 960)]);
        // Without the stop position a key still down counts as held to the end of the loop.
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: true }, &keys, &[]));
        assert_eq!(got.len(), 4, "{got:?}");
        // In one round a stop ends the key where it is, like the end of the range does.
        let one_round = [ev(q, 0, 0x90, 60, 100)];
        let got =
            notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: Some(2 * q), marked: true }, &one_round, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 960)]);
        let events = held;
        // The marks are not gaps: the take is complete.
        let mut e = Engine::default();
        armed(&mut e, "Keys", "none");
        assert_eq!(add_midi_recording(&mut e, range, &events, &[], false).gaps, 0);
    }

    #[test]
    fn a_release_stamped_before_its_press_in_a_pass_that_did_not_loop_is_one_short_note() {
        // The audio callback ran late: the key press was written down at 1 024, and the release,
        // taken by a later callback, at 672. Nothing went round.
        let s = Session::default();
        let range = Range::new(0, 96_000);
        let events = [ev(24_000, 0, 0x90, 60, 100), ev(23_000, 0, 0x80, 60, 40), ev(48_000, 0, 0x90, 64, 100), ev(72_000, 0, 0x80, 64, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &events, &[]));
        assert_eq!(got, [(0, 60, 960, 1, 100, 40), (0, 64, 1920, 960, 100, 0)], "one note at the press, not two around the clip's ends");
        // The same entries from a loop take without marks are a note held across the loop point.
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: false }, &events, &[]));
        assert_eq!(got.iter().filter(|n| n.1 == 60).count(), 2);
        // In a loop take with marks no mark lies between the two: a late callback again.
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: true }, &events, &[]));
        assert_eq!(got, [(0, 60, 960, 1, 100, 40), (0, 64, 1920, 960, 100, 0)]);
        // A key pressed in the last sample of the range starts at the last tick that plays.
        let unmarked = MidiPass::Loop { stopped: None, marked: false };
        // In a take without marks, a key pressed there and released a tick's worth earlier in
        // the next round went round the loop: judged by where it was pressed, not where it starts.
        let round = [ev(95_999, 0, 0x90, 60, 100), ev(95_975, 0, 0x80, 60, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, unmarked, &round, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(0, 3839), (3839, 1)], "the tail and the round after it");
        // Two positions within one tick: a release 22 samples before the press went round too,
        // and a release 22 samples after it is one short note.
        let near = [ev(24_011, 0, 0x90, 60, 100), ev(23_989, 0, 0x80, 60, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, unmarked, &near, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(0, 960), (960, 2880)]);
        let forward = [ev(23_989, 0, 0x90, 60, 100), ev(24_011, 0, 0x80, 60, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, unmarked, &forward, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 1)]);
        // A controller there stays inside the clip too.
        let cc = [ev(95_999, 0, 0xB0, 7, 90)];
        let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &cc, &[]);
        assert_eq!(seq.ctrls.iter().map(|c| c.tick).collect::<Vec<_>>(), [3839]);
        let late = [ev(95_999, 0, 0x90, 60, 100)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &late, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(3839, 1)], "not at the tick of the clip's end");
        // A mark of the loop going round means nothing in a pass that did not loop.
        let marked = [ev(24_000, 0, 0x90, 60, 100), ev(96_000, u8::MAX, MidiTakeEvent::WRAP, 0, 0), ev(48_000, 0, 0x80, 60, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &marked, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 960)]);
    }

    #[test]
    fn the_stop_position_of_a_loop_pass_is_not_one_of_the_capped_entries() {
        // Exactly as many entries as a take keeps, the last a key that is still down at the stop.
        let s = Session::default();
        let range = Range::new(0, 96_000);
        let mut events = vec![ev(-1, 0, 0xB0, 1, 0); MAX_MIDI_TAKE_EVENTS - 1];
        events.push(ev(24_000, 0, 0x90, 60, 100));
        let got =
            notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: Some(48_000), marked: true }, &events, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 960)], "the key ends at the stop, not at the loop end");
        let mut e = Engine::default();
        armed(&mut e, "Keys", "none");
        let inputs = midi_record_inputs(&e);
        let take = add_midi_recording_from(&mut e, range, MidiPass::Loop { stopped: Some(48_000), marked: true }, &events, &[], &inputs, false);
        assert_eq!((take.clips.len(), take.gaps), (1, 0), "and a full take is not reported as cut");
        // One entry more and the take is cut: the stop may be in a round whose mark was cut off,
        // so the key is held to the end of the loop instead of ending early.
        events.push(ev(96_000, u8::MAX, MidiTakeEvent::WRAP, 0, 0));
        let got =
            notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: Some(48_000), marked: true }, &events, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 2880)]);
    }

    #[test]
    fn a_track_taken_out_of_record_during_the_pass_keeps_what_it_recorded() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let range = Range::new(0, 48_000);
        let events = [ev(100, 0, 0x90, 60, 100), ev(200, 0, 0x80, 60, 0)];
        let inputs = midi_record_inputs(&e);
        e.execute("mix.record_arm", &json!({"tracks": [t.0], "value": false})).unwrap();
        let take = add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &[], &inputs, false);
        assert_eq!(take.clips.len(), 1, "it was record-enabled when the pass started");
        assert_eq!(notes_of(&sequence(&e, t, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [60]);
        // A track that was not record-enabled at the start, and is not now, records nothing.
        e.run("edit.undo").unwrap();
        assert!(add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &[], &[], false).clips.is_empty());
        // Nor does one that was put in record during the pass: it would get what was played
        // before that, and nobody waited for the end of its take.
        e.execute("mix.record_arm", &json!({"tracks": [t.0], "value": true})).unwrap();
        assert!(add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &[], &[], false).clips.is_empty());
        assert!(e.session().track(t).unwrap().clips().is_empty());
    }

    #[test]
    fn a_track_records_the_input_it_had_when_the_pass_started() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "MIDI:*:A");
        let other = armed(&mut e, "Other", "MIDI:*:B");
        let devices = ["A".to_string(), "B".to_string()];
        let range = Range::new(0, 48_000);
        let events = [ev(100, 0, 0x90, 60, 100), ev(200, 0, 0x80, 60, 0), ev(300, 1, 0x90, 72, 100), ev(400, 1, 0x80, 72, 0)];
        let inputs = midi_record_inputs(&e);
        assert_eq!(inputs.len(), 2);
        // During the pass the first track is switched to device B.
        e.execute("track.input", &json!({"tracks": [t.0], "input": "MIDI:*:B"})).unwrap();
        let take = add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &devices, &inputs, false);
        assert_eq!(take.clips.len(), 2);
        assert_eq!(notes_of(&sequence(&e, t, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [60], "what it recorded from A is kept");
        assert_eq!(notes_of(&sequence(&e, other, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [72]);
        // The next pass uses the input as it is when that pass starts.
        e.run("edit.undo").unwrap();
        let inputs = midi_record_inputs(&e);
        add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &devices, &inputs, false);
        assert_eq!(notes_of(&sequence(&e, t, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [72]);
    }

    #[test]
    fn notes_keep_their_musical_place_at_any_tempo_and_sample_rate() {
        for (hz, bpm) in [(44_100u32, 120.0), (48_000, 90.0), (96_000, 60.0), (192_000, 174.5)] {
            let mut s = Session::new("t", soundcraft_time::SampleRate::new(hz).unwrap());
            s.tempo.set_tempo(0, bpm).unwrap();
            // The pass starts one quarter note in and lasts four.
            let q = (f64::from(hz) * 60.0 / bpm).round() as i64;
            let range = Range::new(q, 5 * q);
            let events = [
                ev(2 * q, 0, 0x90, 60, 100), // held to the end of the pass
                ev(3 * q, 0, 0x90, 64, 100),
                ev(4 * q, 0, 0x80, 64, 0),
            ];
            let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &events, &[]);
            let near = |got: i64, want: i64| (got - want).abs() <= 1;
            let n = notes_of(&seq);
            assert_eq!(n.len(), 2, "{hz} Hz, {bpm} bpm");
            assert!(near(n[0].2, 960) && near(n[0].3, 2880), "{hz} Hz, {bpm} bpm: the held note ends with the pass: {n:?}");
            assert!(near(n[1].2, 1920) && near(n[1].3, 960), "{hz} Hz, {bpm} bpm: {n:?}");
        }
    }

    #[test]
    fn each_input_filter_kind_keeps_its_own_messages_out_of_the_take() {
        let events = [
            ev(100, 0, 0x90, 60, 100),
            ev(200, 0, 0x80, 60, 0),
            ev(300, 0, 0xA0, 60, 50),
            ev(400, 0, 0xB0, 1, 60),
            ev(500, 0, 0xC0, 5, 0),
            ev(600, 0, 0xD0, 70, 0),
            ev(700, 0, 0xE0, 0, 80),
        ];
        let range = Range::new(0, 48_000);
        // What is left of (notes, poly pressure, controllers, programs, channel pressure, bends).
        let kept = |filter: &[&str]| {
            let mut e = Engine::default();
            let t = armed(&mut e, "Keys", "none");
            e.execute("setup.midi_input_filter", &json!({"filter": filter})).unwrap();
            if add_midi_recording(&mut e, range, &events, &[], false).clips.is_empty() {
                return [0; 6];
            }
            let seq = sequence(&e, t, range);
            let ctrls = |f: &dyn Fn(&CtrlKind) -> bool| seq.ctrls.iter().filter(|c| f(&c.kind)).count();
            [
                seq.notes.len(),
                ctrls(&|k| matches!(k, CtrlKind::PolyPressure(_))),
                ctrls(&|k| matches!(k, CtrlKind::Cc(_))),
                ctrls(&|k| matches!(k, CtrlKind::Program)),
                ctrls(&|k| matches!(k, CtrlKind::ChannelPressure)),
                ctrls(&|k| matches!(k, CtrlKind::PitchBend)),
            ]
        };
        assert_eq!(kept(&[]), [1; 6]);
        for (i, kind) in ["notes", "poly_aftertouch", "cc", "program", "mono_aftertouch", "pitch_bend"].into_iter().enumerate() {
            let mut want = [1; 6];
            want[i] = 0;
            assert_eq!(kept(&[kind]), want, "{kind}");
        }
        assert_eq!(kept(&["notes", "poly_aftertouch", "cc", "program", "mono_aftertouch", "pitch_bend"]), [0; 6], "nothing left: no clip");
        assert_eq!(kept(&["sysex", "realtime"]), [1; 6], "kinds that never reach a take change nothing");
    }

    #[test]
    fn midi_clips_join_the_undo_step_of_the_audio_takes_of_the_same_pass() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let range = Range::new(0, 48_000);
        let events = [ev(100, 0, 0x90, 60, 100), ev(200, 0, 0x80, 60, 0)];
        let clips = |e: &Engine| e.session().track(t).unwrap().clips().len();
        // On its own the take is one undo step called Record.
        let before = e.undo_label().map(str::to_string);
        assert_eq!(add_midi_recording(&mut e, range, &events, &[], false).clips.len(), 1);
        assert_eq!(e.undo_label(), Some("Record"));
        e.run("edit.undo").unwrap();
        assert_eq!((clips(&e), e.undo_label().map(str::to_string)), (0, before.clone()));
        // With audio takes of the same pass, it adds no step of its own: theirs covers it.
        assert_eq!(add_midi_recording(&mut e, range, &events, &[], true).clips.len(), 1);
        assert_eq!((clips(&e), e.undo_label().map(str::to_string)), (1, before));
        // A take that makes no clip makes no step either.
        let mut e = Engine::default();
        armed(&mut e, "Keys", "MIDI:*:Nothing");
        let before = e.undo_label().map(str::to_string);
        assert!(add_midi_recording(&mut e, range, &events, &["Keys".to_string()], false).clips.is_empty());
        assert_eq!(e.undo_label().map(str::to_string), before);
    }

    #[test]
    fn a_device_index_without_a_name_reaches_only_tracks_that_take_every_device() {
        let mut e = Engine::default();
        let all = armed(&mut e, "All", "none");
        let keys = armed(&mut e, "Keys", "MIDI:*:Keys");
        let channel = armed(&mut e, "Ch 3", "MIDI:3:*");
        let range = Range::new(0, 48_000);
        // Device 200 is past the name list; `u8::MAX` is a port that never got an index.
        let events = [ev(100, 200, 0x92, 60, 100), ev(200, 200, 0x82, 60, 0), ev(300, u8::MAX, 0x90, 62, 100), ev(400, u8::MAX, 0x80, 62, 0)];
        let take = add_midi_recording(&mut e, range, &events, &["Keys".to_string()], false);
        assert_eq!(take.clips.len(), 2);
        assert_eq!(notes_of(&sequence(&e, all, range)).len(), 2);
        assert_eq!(notes_of(&sequence(&e, channel, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [60], "its channel, any device");
        assert!(e.session().track(keys).unwrap().clips().is_empty(), "not the named device");
    }

    #[test]
    fn events_past_the_limit_are_left_out_and_the_take_is_reported_incomplete() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let range = Range::new(0, 48_000);
        // The limit's worth of events from before the pass, then two notes: one inside the limit…
        let mut events = vec![ev(-1, 0, 0xB0, 1, 0); MAX_MIDI_TAKE_EVENTS - 2];
        events.extend([ev(100, 0, 0x90, 60, 100), ev(200, 0, 0x80, 60, 0)]);
        let take = add_midi_recording(&mut e, range, &events, &[], false);
        assert_eq!((take.clips.len(), take.gaps), (1, 0), "exactly the limit is complete");
        e.run("edit.undo").unwrap();
        // …and one past it.
        events.extend([ev(300, 0, 0x90, 72, 100), ev(400, 0, 0x80, 72, 0)]);
        let take = add_midi_recording(&mut e, range, &events, &[], false);
        assert_eq!((take.clips.len(), take.gaps), (1, 1), "the cut is reported");
        assert_eq!(notes_of(&sequence(&e, t, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [60]);
    }
}
