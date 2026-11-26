//! MIDI played during a record pass, turned into MIDI clips.

use super::{Clip, Engine, Range, Samples, Session, TrackId, TrackKind};
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{ClipId, MidiInput, Route};
use std::collections::HashMap;

mod merge;

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
    /// The clips made: one per track that received something, or, as the record mode says
    /// ([`MidiRecordStyle`]), a take per round of Loop Record or the clips that fill the gaps
    /// between the clips merged into.
    pub clips: Vec<ClipId>,
    /// Clips that were on the tracks already and that MIDI Merge recorded into.
    pub merged: Vec<ClipId>,
    /// Places within the pass where input was dropped ([`MidiTakeEvent::LOST`]): the take is
    /// incomplete when this is not zero.
    pub gaps: usize,
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
#[cfg(test)]
fn midi_take_sequence(s: &Session, input: &MidiInput<'_>, range: Range, pass: MidiPass, events: &[MidiTakeEvent], devices: &[String]) -> Sequence {
    midi_take_rounds(s, input, range, pass, events, devices, false).into_iter().next().map(|r| r.seq).unwrap_or_default()
}

/// What one round of a record pass recorded on a track ([`midi_take_rounds`]).
#[derive(Debug, Clone, Default, PartialEq)]
struct RoundTake {
    /// In ticks from the start of the range.
    seq: Sequence,
    /// Whether the track received new material in this round: a note played or a controller
    /// moved. The part of a key held over from the round before, a lone release or dropped
    /// input are not new material (Pro Tools makes a Loop Record take "when new MIDI material is
    /// received", Reference Guide p.826).
    fresh: bool,
}

/// [`midi_take_sequence`], round by round. With `split` and a loop pass whose rounds are marked,
/// each round of the loop is a take of its own: a note belongs to the round of its key press
/// ("Note Ons can never precede the beginning of a clip", Reference Guide p.819), a key held into
/// the next rounds sounds on in them from the loop start, and a controller belongs to the round it
/// was moved in. Otherwise (and for any other pass) everything is in one take, laid out as
/// [`midi_take_sequence`] says.
fn midi_take_rounds(
    s: &Session,
    input: &MidiInput<'_>,
    range: Range,
    pass: MidiPass,
    events: &[MidiTakeEvent],
    devices: &[String],
    split: bool,
) -> Vec<RoundTake> {
    let split = split && matches!(pass, MidiPass::Loop { marked: true, .. });
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(range.start, sr);
    let tick = |at: Samples| s.tempo.samples_to_ticks(at, sr).saturating_sub(base).max(0);
    let end_tick = tick(range.end).max(1);
    let looped = matches!(pass, MidiPass::Loop { .. });
    // Whether a release earlier on the timeline than its press says that the loop went round.
    let inferred = matches!(pass, MidiPass::Loop { marked: false, .. });
    // The last tick a note can start at and still be played: a key pressed within the last
    // samples of the range may round to the tick of its end, where the clip no longer plays.
    let last_start = end_tick.saturating_sub(1).max(0);
    let mut takes = vec![RoundTake::default()];
    // The take a round goes into: its own when split, else the one take.
    let slot = |takes: &mut Vec<RoundTake>, round: u32| -> usize {
        let i = if split { usize::try_from(round).unwrap_or(usize::MAX) } else { 0 };
        if takes.len() <= i {
            takes.resize_with(i + 1, RoundTake::default);
        }
        i
    };
    // The round of the loop being played: it goes up at every `WRAP`.
    let mut round = 0u32;
    // Keys down: (device, channel, pitch) -> (start tick, velocity, round, the sample the key was
    // pressed at). Whether the loop went round between a press and its release is judged by
    // their samples: two positions a few samples apart may share a tick, and a press in the
    // last samples of the range starts its note a tick earlier so that it plays.
    let mut down: HashMap<(u8, u8, u8), (i64, u8, u32, Samples)> = HashMap::new();
    let close = |takes: &mut Vec<RoundTake>,
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
            let i = slot(takes, since);
            takes[i].seq.notes.push(note(start, end, release));
            return;
        }
        if split {
            // Each round its own take: the key sounds from its press to the end of the loop, over
            // the whole loop in the rounds it was held through, and from the loop start to its
            // release in the round it was let go in.
            let i = slot(takes, since);
            takes[i].seq.notes.push(note(start, end_tick, 64));
            for r in since.saturating_add(1)..round {
                let i = slot(takes, r);
                takes[i].seq.notes.push(note(0, end_tick, 64));
            }
            if end > 0 {
                let i = slot(takes, round);
                takes[i].seq.notes.push(note(0, end, release));
            }
            return;
        }
        let seq = &mut takes[0].seq;
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
                    close(&mut takes, key, open, at, 64, round, e.at);
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
        let moved = match e.status & 0xF0 {
            0x90 if d2 > 0 => {
                // The same key again without a release: the first note ends here.
                if let Some(open) = down.insert(key, (at.min(last_start), d2, round, e.at)) {
                    close(&mut takes, key, open, at, 64, round, e.at);
                }
                let i = slot(&mut takes, round);
                takes[i].fresh = true;
                None
            }
            0x80 | 0x90 => {
                if let Some(open) = down.remove(&key) {
                    close(&mut takes, key, open, at, if e.status & 0xF0 == 0x80 { d2 } else { 64 }, round, e.at);
                }
                None
            }
            0xA0 => Some(ctrl(CtrlKind::PolyPressure(d1), i32::from(d2))),
            0xB0 => Some(ctrl(CtrlKind::Cc(d1), i32::from(d2))),
            0xC0 => Some(ctrl(CtrlKind::Program, i32::from(d1))),
            0xD0 => Some(ctrl(CtrlKind::ChannelPressure, i32::from(d1))),
            0xE0 => Some(ctrl(CtrlKind::PitchBend, (i32::from(d2) << 7 | i32::from(d1)) - 8192)),
            _ => None,
        };
        if let Some(c) = moved {
            let i = slot(&mut takes, round);
            takes[i].seq.ctrls.push(c);
            takes[i].fresh = true;
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
        close(&mut takes, key, open, stop_at.map_or(end_tick, tick), 64, round, stop_at.unwrap_or(range.end));
    }
    // The round the pass stopped in is there even when nothing was played in it.
    slot(&mut takes, round);
    for t in &mut takes {
        t.seq.sort();
    }
    takes
}

/// Most events of one MIDI record pass that are kept.
pub const MAX_MIDI_TAKE_EVENTS: usize = 1 << 20;

/// Turn the MIDI played during a record pass that went once over `range` into a clip on every
/// MIDI and instrument track that is record-enabled now, each taking the events of its MIDI input
/// (device and channel). `devices` names the devices by [`MidiTakeEvent::device`]. A track that
/// received nothing gets no clip. One undo step ("MIDI Recording", as Pro Tools names it, Reference
/// Guide p.820); with `same_pass` the clips join the step the audio takes of this pass already
/// made ("Record"). The result says where input was dropped
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
    record_midi(e, range, pass, events, devices, inputs, Write::Piled { same_pass })
}

/// How MIDI Merge Release mode writes controllers recorded in MIDI Merge mode (Reference Guide
/// p.814, p.176). It applies to the continuous controllers: control changes, pitch bend and
/// channel (mono) aftertouch, the kinds Pro Tools lists as continuous controller events
/// (p.1042–1043). Program changes and polyphonic aftertouch are merged as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeRelease {
    /// "Continues writing the last value until the next event even if you are still recording":
    /// the existing events of a controller are replaced from the first to the last value played,
    /// and after that the existing ones play on.
    #[default]
    LatchUntilNextEvent,
    /// "Continues writing the last value. Any existing events are overwritten until you stop
    /// recording": from the first value played to the stop. In a loop pass the later rounds go
    /// on writing the last value over the whole loop.
    Latch,
}

impl MergeRelease {
    pub const ALL: [Self; 2] = [Self::Latch, Self::LatchUntilNextEvent];

    /// The text shown for the mode.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Latch => "Latch",
            Self::LatchUntilNextEvent => "Latch Until Next Event",
        }
    }

    /// The command and settings identifier of the mode.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Latch => "latch",
            Self::LatchUntilNextEvent => "latch_until_next_event",
        }
    }

    /// The mode an identifier names.
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }
}

/// How the MIDI of a record pass is written into the tracks: the record mode it ran in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiRecordStyle {
    /// Loop Record: every round of the loop in which a track received new material is a take of
    /// its own, the length of the loop, replacing what the loop covers without deleting it: the
    /// first on the track's playlist, each further one on a new playlist that becomes the
    /// track's. MIDI Merge has no effect (Reference Guide p.774, p.813, p.825–826).
    pub loop_record: bool,
    /// MIDI Merge, round by round of the pass (one entry for a pass that did not loop): on, what
    /// was played is merged into the MIDI already on the track; off, it replaces what the range
    /// covers. A round past the last entry has the last entry's state; none at all is off.
    pub merge: Vec<bool>,
    /// How merged controllers are written.
    pub release: MergeRelease,
    /// The pass also recorded audio, whose takes already made its undo step: the MIDI joins it.
    pub same_pass: bool,
    /// Where a pass punched in while the loop was playing started (where the audio engine's
    /// playhead stood when it began): its first round covers only from there, so a replacing
    /// first round leaves what lies before the punch-in point. Should the point be the UI's own
    /// playhead instead (used when the audio engine did not say), which may lag behind, the round
    /// starts no later than the first thing it recorded.
    pub punched_in: Option<Samples>,
}

impl MidiRecordStyle {
    /// Whether MIDI Merge was on in `round`.
    fn merges(&self, round: usize) -> bool {
        !self.loop_record && self.merge.get(round).or(self.merge.last()).copied().unwrap_or(false)
    }
}

/// [`add_midi_recording_from`], recorded as `style` says ([`MidiRecordStyle`]). A loop pass is
/// taken round by round: with MIDI Merge each round is merged into the track in turn, so what
/// was played in a round counts as already there for the next (drum machine style loop
/// recording, Reference Guide p.824–825). Without it the last round that received new material
/// replaces the range, and a round with nothing new played changes nothing ("each subsequent take
/// destructively replaces the previous take", p.824). The pass is one undo step, with every take
/// and playlist it made.
pub fn add_midi_recording_with(
    e: &mut Engine,
    range: Range,
    pass: MidiPass,
    events: &[MidiTakeEvent],
    devices: &[String],
    inputs: &[(TrackId, Route)],
    style: &MidiRecordStyle,
) -> MidiTake {
    record_midi(e, range, pass, events, devices, inputs, Write::Styled(style))
}

/// How [`record_midi`] writes a pass.
enum Write<'a> {
    /// Every round of the pass in one clip that replaces the range ([`add_midi_recording_from`]).
    Piled { same_pass: bool },
    /// As the record mode says ([`add_midi_recording_with`]).
    Styled(&'a MidiRecordStyle),
}

fn record_midi(
    e: &mut Engine,
    range: Range,
    pass: MidiPass,
    events: &[MidiTakeEvent],
    devices: &[String],
    inputs: &[(TrackId, Route)],
    write: Write<'_>,
) -> MidiTake {
    let (style, same_pass) = match write {
        Write::Piled { same_pass } => (None, same_pass),
        Write::Styled(style) => (Some(style), style.same_pass),
    };
    let before = e.session_arc();
    let mut out = MidiTake::default();
    if range.len() <= 0 {
        return out;
    }
    out.gaps =
        events.iter().take(MAX_MIDI_TAKE_EVENTS).filter(|ev| ev.status == MidiTakeEvent::LOST && ev.at >= range.start && ev.at < range.end).count()
            + usize::from(events.len() > MAX_MIDI_TAKE_EVENTS);
    let takes: Vec<(TrackId, String, Vec<RoundTake>)> = {
        let s = e.session();
        s.tracks
            .iter()
            .filter_map(|t| {
                let (_, route) = inputs.iter().find(|(id, _)| *id == t.id)?;
                Some((t.id, t.name.clone(), midi_take_rounds(s, &MidiInput::of(route), range, pass, events, devices, style.is_some())))
            })
            .collect()
    };
    let changed = match style {
        None => {
            for (tid, name, rounds) in takes {
                let Some(seq) = rounds.into_iter().next().map(|r| r.seq).filter(|q| !q.notes.is_empty() || !q.ctrls.is_empty()) else { continue };
                let s = e.session_mut();
                let cid = s.new_clip_id();
                crate::edit::place_clip(s, tid, Clip::midi(cid, name, range.start, range.len(), seq));
                out.clips.push(cid);
            }
            !out.clips.is_empty()
        }
        Some(style) => {
            let stop = match pass {
                MidiPass::Loop { stopped: Some(at), .. } if events.len() <= MAX_MIDI_TAKE_EVENTS => at.clamp(range.start, range.end),
                _ => range.end,
            };
            let mut changed = false;
            for (tid, name, rounds) in takes {
                let s = e.session_mut();
                changed |= if style.loop_record && rounds.len() > 1 {
                    place_round_takes(s, tid, &name, range, &rounds, &mut out)
                } else {
                    write_rounds(s, tid, &name, (range, stop), &rounds, style, &mut out)
                };
            }
            changed
        }
    };
    if changed && !same_pass {
        e.push_undo("MIDI Recording", before);
    }
    out
}

/// Loop Record: a take of the loop's length for every round in which the track received new
/// material, the first on the track's playlist, each further one on a new playlist named after
/// the track and numbered, which becomes the track's (as audio loop recording lays out its
/// passes). Whether any was made.
fn place_round_takes(s: &mut Session, tid: TrackId, name: &str, range: Range, rounds: &[RoundTake], out: &mut MidiTake) -> bool {
    let mut first = true;
    for r in rounds.iter().filter(|r| r.fresh) {
        if !first && let Some(t) = s.track_mut(tid) {
            t.playlists.push(soundcraft_model::Playlist::new(format!("{}.{:02}", t.name, t.playlists.len() + 1)));
            t.active_playlist = t.playlists.len() - 1;
        }
        first = false;
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, tid, Clip::midi(cid, name, range.start, range.len(), r.seq.clone()));
        out.clips.push(cid);
    }
    !first
}

/// The rounds of a pass written one after another, each merged or replacing as MIDI Merge was
/// in it ([`MidiRecordStyle::merges`]). `stop` is where the pass stopped: the last round ends
/// there. The clips made and merged into are added to `out`. Whether the track changed.
fn write_rounds(
    s: &mut Session,
    tid: TrackId,
    name: &str,
    (range, stop): (Range, Samples),
    rounds: &[RoundTake],
    style: &MidiRecordStyle,
    out: &mut MidiTake,
) -> bool {
    let looped = rounds.len() > 1;
    let mut latch = merge::Latch::new(style.release);
    let mut changed = false;
    // This track's clips: those of the tracks written before it stay in `out` as they are.
    let (mut clips, mut merged) = (Vec::new(), Vec::new());
    for (i, r) in rounds.iter().enumerate() {
        let last = i + 1 == rounds.len();
        let empty = r.seq.notes.is_empty() && r.seq.ctrls.is_empty();
        // The last round of a loop pass ends at the stop, the first one too when the pass stopped
        // before the loop went round. A round stopped at its very start covered nothing and
        // changes nothing.
        let until = if last { stop } else { range.end };
        if until <= range.start {
            continue;
        }
        if style.merges(i) {
            let (made, into) = merge::merge_take(s, tid, name, range, until, &r.seq, &mut latch);
            changed |= !made.is_empty() || !into.is_empty();
            clips.extend(made);
            for m in into {
                if !merged.contains(&m) {
                    merged.push(m);
                }
            }
        } else if !empty && (r.fresh || !looped) {
            // A last loop round cut short by Stop replaces only what it covered: what was there
            // after the stop stays (Replace overwrites "within the record range", Reference Guide
            // p.823–824). A Loop Record take is the loop's length (p.826).
            let end = if style.loop_record { range.end } else { until };
            // A first round punched in while the loop played covers only from the punch-in
            // point: what lies before it was not recorded over and stays (p.819). That point is
            // where the audio engine began the pass; should it be the UI's playhead instead,
            // which may lag behind (even by a round), the round starts no later than the first
            // thing it recorded.
            let start = match style.punched_in {
                Some(at) if i == 0 && !style.loop_record => {
                    let first = r.seq.notes.iter().map(|n| n.start).chain(r.seq.ctrls.iter().map(|c| c.tick)).min();
                    let base = s.tempo.samples_to_ticks(range.start, s.sample_rate);
                    let first = first.map_or(at, |t| s.tempo.tick_to_samples(base.saturating_add(t), s.sample_rate));
                    at.min(first).clamp(range.start, end)
                }
                _ => range.start,
            };
            if end <= start {
                continue;
            }
            let shift = s.tempo.samples_to_ticks(start, s.sample_rate) - s.tempo.samples_to_ticks(range.start, s.sample_rate);
            let mut seq = r.seq.clone();
            if shift > 0 {
                for n in &mut seq.notes {
                    n.start = (n.start - shift).max(0);
                }
                for c in &mut seq.ctrls {
                    c.tick = (c.tick - shift).max(0);
                }
            }
            latch = merge::Latch::new(style.release);
            let cid = s.new_clip_id();
            crate::edit::place_clip(s, tid, Clip::midi(cid, name, start, end - start, seq));
            clips.push(cid);
            changed = true;
        }
    }
    // A later round may have replaced what an earlier one made or merged into.
    let alive = |id: &ClipId| s.track(tid).is_some_and(|t| t.clips().iter().any(|c| c.id == *id));
    clips.retain(alive);
    merged.retain(|id| alive(id) && !clips.contains(id));
    out.clips.extend(clips);
    out.merged.extend(merged);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_model::ClipContent;

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
        // On its own the take is one undo step called MIDI Recording (p.820).
        let before = e.undo_label().map(str::to_string);
        assert_eq!(add_midi_recording(&mut e, range, &events, &[], false).clips.len(), 1);
        assert_eq!(e.undo_label(), Some("MIDI Recording"));
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

    fn style(loop_record: bool, merge: &[bool], release: MergeRelease) -> MidiRecordStyle {
        MidiRecordStyle { loop_record, merge: merge.to_vec(), release, same_pass: false, punched_in: None }
    }

    /// Record `events` on every record-enabled track as `style` says.
    fn record(e: &mut Engine, range: Range, pass: MidiPass, events: &[MidiTakeEvent], style: &MidiRecordStyle) -> MidiTake {
        let inputs = midi_record_inputs(e);
        add_midi_recording_with(e, range, pass, events, &[], &inputs, style)
    }

    fn midi_of(c: &Clip) -> &Sequence {
        let ClipContent::Midi { sequence } = &c.content else { panic!("not a MIDI clip") };
        sequence
    }

    /// The pitches of the notes in every clip of the track's playlist, clip by clip.
    fn pitches(e: &Engine, t: TrackId) -> Vec<Vec<u8>> {
        e.session().track(t).unwrap().clips().iter().map(|c| midi_of(c).notes.iter().map(|n| n.pitch).collect()).collect()
    }

    const Q: Samples = 24_000;
    const LOOP: Range = Range { start: 0, end: 4 * Q };
    const WRAP: MidiTakeEvent = MidiTakeEvent { at: 4 * Q, device: u8::MAX, status: MidiTakeEvent::WRAP, data1: 0, data2: 0 };

    #[test]
    fn loop_record_makes_a_take_for_each_round_that_received_new_material() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let events = [
            ev(Q, 0, 0x90, 60, 100), // round 1: a note…
            ev(2 * Q, 0, 0x80, 60, 0),
            ev(3 * Q, 0, 0x90, 67, 100), // …and a key held into round 2
            WRAP,
            ev(Q, 0, 0x80, 67, 0), // round 2: only the release of that key
            WRAP,
            ev(Q, 0, 0x90, 64, 100), // round 3: another note
            ev(2 * Q, 0, 0x80, 64, 0),
            WRAP,
            ev(Q / 2, 0, 0xB0, 1, 90), // round 4, stopped half way: a controller only
        ];
        let pass = MidiPass::Loop { stopped: Some(2 * Q), marked: true };
        let before = e.undo_label().map(str::to_string);
        for merge in [false, true] {
            // MIDI Merge has no effect in Loop Record (Reference Guide p.813).
            let take = record(&mut e, LOOP, pass, &events, &style(true, &[merge], MergeRelease::default()));
            assert_eq!(take.clips.len(), 3, "rounds 1, 3 and 4; round 2 played nothing new");
            let track = e.session().track(t).unwrap();
            let names: Vec<&str> = track.playlists.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names.len(), 3, "{names:?}");
            assert!(names[1].starts_with("Keys.") && names[2].starts_with("Keys.") && names[1] != names[2], "{names:?}");
            assert_eq!(track.active_playlist, 2, "the last take is the track's");
            let takes: Vec<&Clip> = track.playlists.iter().flat_map(|p| p.clips.iter()).collect();
            assert!(takes.iter().all(|c| c.range() == LOOP), "every take is the loop");
            let notes: Vec<Vec<u8>> = takes.iter().map(|c| midi_of(c).notes.iter().map(|n| n.pitch).collect()).collect();
            // Round 1 holds its note and the part of the key held to the loop end; the part held
            // into round 2 made no take of its own.
            assert_eq!(notes, [vec![60, 67], vec![64], vec![]]);
            assert_eq!(midi_of(takes[2]).ctrls.len(), 1);
            // One undo step takes back every take and playlist of the pass.
            assert_eq!(e.undo_label(), Some("MIDI Recording"));
            e.run("edit.undo").unwrap();
            let track = e.session().track(t).unwrap();
            assert_eq!((track.playlists.len(), track.clips().len()), (1, 0));
            assert_eq!(e.undo_label().map(str::to_string), before);
        }
    }

    #[test]
    fn a_loop_record_take_replaces_what_the_loop_covers_and_each_track_counts_its_own_input() {
        let mut e = Engine::default();
        let a = armed(&mut e, "A", "MIDI:*:A");
        let b = armed(&mut e, "B", "MIDI:*:B");
        let devices = ["A".to_string(), "B".to_string()];
        // Something on A's playlist already.
        add_midi_recording(&mut e, LOOP, &[ev(0, 0, 0x90, 50, 100), ev(Q, 0, 0x80, 50, 0)], &devices, false);
        assert_eq!(e.session().track(a).unwrap().clips().len(), 1);
        let events = [ev(Q, 1, 0x90, 60, 100), ev(2 * Q, 1, 0x80, 60, 0), WRAP, ev(Q, 0, 0x90, 62, 100), ev(2 * Q, 0, 0x80, 62, 0)];
        let inputs = midi_record_inputs(&e);
        let pass = MidiPass::Loop { stopped: Some(3 * Q), marked: true };
        let take = add_midi_recording_with(&mut e, LOOP, pass, &events, &devices, &inputs, &style(true, &[], MergeRelease::default()));
        assert_eq!(take.clips.len(), 2);
        // A got material in round 2 only: one take, on its own playlist, in place of what was
        // there; B in round 1 only.
        assert_eq!(pitches(&e, a), [vec![62]]);
        assert_eq!(pitches(&e, b), [vec![60]]);
        assert_eq!((e.session().track(a).unwrap().playlists.len(), e.session().track(b).unwrap().playlists.len()), (1, 1));
    }

    #[test]
    fn a_replace_round_cut_short_by_stop_keeps_what_the_earlier_round_played_after_the_stop() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let events = [
            ev(Q / 2, 0, 0x90, 60, 100), // round 1: early and late notes
            ev(Q, 0, 0x80, 60, 0),
            ev(3 * Q, 0, 0x90, 72, 100),
            ev(3 * Q + Q / 4, 0, 0x80, 72, 0),
            WRAP,
            ev(Q / 2, 0, 0x90, 62, 100), // round 2, stopped at 1.5 Q: one note
            ev(Q, 0, 0x80, 62, 0),
        ];
        let stop = Q + Q / 2;
        let pass = MidiPass::Loop { stopped: Some(stop), marked: true };
        record(&mut e, LOOP, pass, &events, &style(false, &[false], MergeRelease::default()));
        let track = e.session().track(t).unwrap();
        let clips: Vec<(Samples, Samples, Vec<u8>)> =
            track.clips().iter().map(|c| (c.start, c.range().end, midi_of(c).notes.iter().map(|n| n.pitch).collect())).collect();
        // Round 2 replaces the loop up to the stop; round 1's late note, after the stop, stays.
        assert_eq!(clips, [(0, stop, vec![62]), (stop, LOOP.end, vec![72])]);
        // Stopped at the wrap itself: the new round covered nothing and changes nothing.
        e.run("edit.undo").unwrap();
        let mut at_wrap = events[..5].to_vec();
        at_wrap.push(ev(0, 0, 0xB0, 1, 9));
        record(&mut e, LOOP, MidiPass::Loop { stopped: Some(0), marked: true }, &at_wrap, &style(false, &[false], MergeRelease::default()));
        assert_eq!(pitches(&e, t), [vec![60, 72]]);
    }

    #[test]
    fn loop_playback_with_midi_merge_layers_the_rounds_in_one_clip() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Drums", "none");
        let events = [
            ev(0, 0, 0x99, 42, 100), // hi-hats in round 1
            ev(Q / 4, 0, 0x89, 42, 0),
            ev(2 * Q, 0, 0x99, 42, 100),
            ev(2 * Q + Q / 4, 0, 0x89, 42, 0),
            WRAP,
            ev(Q, 0, 0x99, 36, 100), // the kick in round 2
            ev(Q + Q / 4, 0, 0x89, 36, 0),
        ];
        let pass = MidiPass::Loop { stopped: Some(2 * Q), marked: true };
        let take = record(&mut e, LOOP, pass, &events, &style(false, &[true], MergeRelease::default()));
        assert_eq!(take.clips.len(), 1);
        let mut got = pitches(&e, t);
        got[0].sort_unstable();
        assert_eq!(got, [vec![36, 42, 42]], "both rounds, one clip");
        // Without MIDI Merge the last round that played something replaces the earlier ones; a
        // round with nothing new played changes nothing.
        e.run("edit.undo").unwrap();
        let mut silent_last = events.to_vec();
        silent_last.push(WRAP);
        record(&mut e, LOOP, pass, &silent_last, &style(false, &[false], MergeRelease::default()));
        assert_eq!(pitches(&e, t), [vec![36]]);
        // MIDI Merge switched on during round 2: round 2 is merged into round 1.
        e.run("edit.undo").unwrap();
        record(&mut e, LOOP, pass, &silent_last, &style(false, &[false, true], MergeRelease::default()));
        let mut got = pitches(&e, t);
        got[0].sort_unstable();
        assert_eq!(got, [vec![36, 42, 42]]);
    }

    #[test]
    fn a_key_held_over_the_loop_point_in_merge_mode_covers_the_loop_once() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        // Pressed in round 1 at three quarters, released in round 3 at one quarter.
        let events = [ev(3 * Q, 0, 0x90, 60, 100), WRAP, WRAP, ev(Q, 0, 0x80, 60, 0)];
        record(&mut e, LOOP, MidiPass::Loop { stopped: Some(2 * Q), marked: true }, &events, &style(false, &[true], MergeRelease::default()));
        let clip = e.session().track(t).unwrap().clips()[0].clone();
        let mut spans: Vec<(i64, i64)> = midi_of(&clip).notes.iter().map(|n| (n.start, n.end())).collect();
        spans.sort_unstable();
        assert_eq!(spans, [(0, 2880), (2880, 3840)], "no two notes of the key overlap");
    }

    /// A track with one MIDI clip over the first two quarters: notes C4 and E4 and a ramp of
    /// controller 1, controller 7 and pitch bend events.
    fn with_a_clip(e: &mut Engine) -> (TrackId, ClipId) {
        let t = armed(e, "Keys", "none");
        let mut events = vec![ev(0, 0, 0x90, 60, 100), ev(Q, 0, 0x80, 60, 0), ev(Q, 0, 0x90, 64, 100), ev(2 * Q - 1, 0, 0x80, 64, 0)];
        for i in 0..8u8 {
            let at = i64::from(i) * Q / 4;
            events.extend([ev(at, 0, 0xB0, 1, i * 16), ev(at, 0, 0xE0, 0, 64 + i), ev(at, 0, 0xB0, 7, 100)]);
        }
        add_midi_recording(e, Range::new(0, 2 * Q), &events, &[], false);
        let host = e.session().track(t).unwrap().clips()[0].id;
        (t, host)
    }

    #[test]
    fn midi_merge_writes_into_the_clips_there_and_fills_only_the_gaps() {
        let mut e = Engine::default();
        let (t, host) = with_a_clip(&mut e);
        let before = e.session().track(t).unwrap().clips()[0].clone();
        // A punch from the second quarter to the fourth: a note inside the clip, one after it.
        let range = Range::new(Q, 4 * Q);
        let events =
            [ev(Q + Q / 2, 0, 0x90, 72, 100), ev(Q + Q / 2 + 100, 0, 0x80, 72, 0), ev(3 * Q, 0, 0x90, 74, 100), ev(3 * Q + 100, 0, 0x80, 74, 0)];
        let take = record(&mut e, range, MidiPass::Linear, &events, &style(false, &[true], MergeRelease::default()));
        assert_eq!(take.merged, [host]);
        let clips = e.session().track(t).unwrap().clips().to_vec();
        assert_eq!(clips.len(), 2);
        assert_eq!((clips[0].id, clips[0].range()), (host, before.range()), "the clip keeps its place and length");
        assert_eq!(clips[1].range(), Range::new(2 * Q, 4 * Q), "the new clip fills the gap and nothing else");
        assert_eq!(take.clips, [clips[1].id]);
        assert_eq!(pitches(&e, t), [vec![60, 64, 72], vec![74]], "what was there plays on");
        // Note positions inside the clip are counted from its start.
        assert_eq!(midi_of(&clips[0]).notes.iter().find(|n| n.pitch == 72).map(|n| n.start), Some(1440));
        e.run("edit.undo").unwrap();
        assert_eq!(e.session().track(t).unwrap().clips(), [before]);
    }

    #[test]
    fn a_new_note_on_a_key_that_is_sounding_ends_the_earlier_one() {
        let mut e = Engine::default();
        let (t, _) = with_a_clip(&mut e);
        // C4 sounds over the first quarter; it is played again half way through it.
        let events = [ev(Q / 2, 0, 0x90, 60, 90), ev(Q, 0, 0x80, 60, 0)];
        record(&mut e, Range::new(0, 2 * Q), MidiPass::Linear, &events, &style(false, &[true], MergeRelease::default()));
        let clip = e.session().track(t).unwrap().clips()[0].clone();
        let c4: Vec<(i64, i64, u8)> = midi_of(&clip).notes.iter().filter(|n| n.pitch == 60).map(|n| (n.start, n.end(), n.velocity)).collect();
        assert_eq!(c4, [(0, 480, 100), (480, 960, 90)]);
    }

    #[test]
    fn the_release_mode_decides_how_far_new_controller_values_replace_the_old() {
        // New values of controller 1 and pitch bend from the first eighth to the second quarter
        // of a punch over the whole clip; controller 7 and a program change are not played over.
        let events = [ev(Q / 2, 0, 0xB0, 1, 5), ev(Q, 0, 0xB0, 1, 6), ev(Q / 2, 0, 0xE0, 0, 0), ev(Q, 0, 0xE0, 0, 1), ev(Q, 0, 0xC0, 9, 0)];
        let range = Range::new(0, 2 * Q);
        let run = |release| {
            let mut e = Engine::default();
            let (t, _) = with_a_clip(&mut e);
            record(&mut e, range, MidiPass::Linear, &events, &style(false, &[true], release));
            let clip = e.session().track(t).unwrap().clips()[0].clone();
            midi_of(&clip).ctrls.clone()
        };
        let ticks = |ctrls: &[CtrlEvent], kind: CtrlKind| ctrls.iter().filter(|c| c.kind == kind).map(|c| c.tick).collect::<Vec<_>>();
        // Latch Until Next Event: the old values between the first and last new value give way;
        // after it the old ramp plays on.
        let lune = run(MergeRelease::LatchUntilNextEvent);
        assert_eq!(ticks(&lune, CtrlKind::Cc(1)), [0, 240, 480, 960, 1200, 1440, 1680]);
        assert_eq!(ticks(&lune, CtrlKind::PitchBend), ticks(&lune, CtrlKind::Cc(1)), "pitch bend is a continuous controller too");
        // Latch: from the first new value to the stop.
        let latch = run(MergeRelease::Latch);
        assert_eq!(ticks(&latch, CtrlKind::Cc(1)), [0, 240, 480, 960]);
        assert_eq!(ticks(&latch, CtrlKind::PitchBend), [0, 240, 480, 960]);
        for ctrls in [&lune, &latch] {
            assert_eq!(ticks(ctrls, CtrlKind::Cc(7)).len(), 8, "a controller not played over is kept");
            assert_eq!(ticks(ctrls, CtrlKind::Program), [960], "a program change is merged as it is");
        }
    }

    #[test]
    fn latch_goes_on_writing_the_last_value_over_the_later_rounds_of_a_loop() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        // Controller 1 moved in round 1 only; round 2 plays a note.
        let events = [ev(Q, 0, 0xB0, 1, 10), ev(2 * Q, 0, 0xB0, 1, 20), WRAP, ev(Q / 2, 0, 0x90, 60, 100), ev(Q, 0, 0x80, 60, 0)];
        let pass = MidiPass::Loop { stopped: Some(3 * Q), marked: true };
        let cc1 = |e: &Engine| {
            let clip = e.session().track(t).unwrap().clips()[0].clone();
            midi_of(&clip).ctrls.iter().filter(|c| c.kind == CtrlKind::Cc(1)).map(|c| (c.tick, c.value)).collect::<Vec<_>>()
        };
        record(&mut e, LOOP, pass, &events, &style(false, &[true], MergeRelease::Latch));
        assert_eq!(cc1(&e), [(0, 20)], "round 2 holds the last value over the whole loop");
        e.run("edit.undo").unwrap();
        record(&mut e, LOOP, pass, &events, &style(false, &[true], MergeRelease::LatchUntilNextEvent));
        assert_eq!(cc1(&e), [(960, 10), (1920, 20)], "round 2 leaves round 1's values alone");
    }

    #[test]
    fn an_edit_locked_clip_is_not_merged_into() {
        let mut e = Engine::default();
        let (t, host) = with_a_clip(&mut e);
        e.session_mut().find_clip_mut(host).unwrap().edit_locked = true;
        let before = e.session().track(t).unwrap().clips()[0].clone();
        let events = [ev(Q / 2, 0, 0x90, 72, 100), ev(Q, 0, 0x80, 72, 0)];
        let take = record(&mut e, Range::new(0, 2 * Q), MidiPass::Linear, &events, &style(false, &[true], MergeRelease::default()));
        assert!(take.merged.is_empty());
        let clips = e.session().track(t).unwrap().clips().to_vec();
        assert!(clips.contains(&before), "the locked clip is as it was");
        assert_eq!(take.clips.len(), 1, "what was played is in a new clip over it");
    }

    #[test]
    fn without_midi_merge_a_pass_in_a_line_replaces_the_range_as_before() {
        let mut e = Engine::default();
        let (t, _) = with_a_clip(&mut e);
        let events = [ev(Q / 2, 0, 0x90, 72, 100), ev(Q, 0, 0x80, 72, 0)];
        let take = record(&mut e, Range::new(0, 2 * Q), MidiPass::Linear, &events, &style(false, &[false], MergeRelease::default()));
        assert!(take.merged.is_empty());
        assert_eq!(pitches(&e, t), [vec![72]]);
    }

    #[test]
    fn a_first_loop_round_stopped_before_the_wrap_replaces_only_up_to_the_stop() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        // On the track already: a note early in the loop and one late in it.
        add_midi_recording(
            &mut e,
            LOOP,
            &[ev(Q / 4, 0, 0x90, 50, 100), ev(Q / 2, 0, 0x80, 50, 0), ev(3 * Q, 0, 0x90, 72, 100), ev(3 * Q + Q / 4, 0, 0x80, 72, 0)],
            &[],
            false,
        );
        // Loop Playback, MIDI Merge off: stopped half way through the second quarter of round 1.
        let events = [ev(Q / 2, 0, 0x90, 62, 100), ev(Q, 0, 0x80, 62, 0)];
        let stop = Q + Q / 2;
        let pass = MidiPass::Loop { stopped: Some(stop), marked: true };
        record(&mut e, LOOP, pass, &events, &style(false, &[false], MergeRelease::default()));
        let spans = |e: &Engine| -> Vec<(Samples, Samples, Vec<u8>)> {
            e.session()
                .track(t)
                .unwrap()
                .clips()
                .iter()
                .map(|c| (c.start, c.range().end, midi_of(c).notes.iter().map(|n| n.pitch).collect()))
                .collect()
        };
        assert_eq!(spans(&e), [(0, stop, vec![62]), (stop, LOOP.end, vec![72])], "the note after the stop stays");
        // A Loop Record take is the loop's length however early the pass stopped.
        e.run("edit.undo").unwrap();
        record(&mut e, LOOP, pass, &events, &style(true, &[], MergeRelease::default()));
        assert_eq!(spans(&e), [(0, LOOP.end, vec![62])]);
    }

    #[test]
    fn each_track_keeps_its_clips_in_the_result_when_several_record() {
        let mut e = Engine::default();
        armed(&mut e, "A", "MIDI:*:A");
        armed(&mut e, "B", "MIDI:*:B");
        let devices = ["A".to_string(), "B".to_string()];
        let events = [ev(0, 0, 0x90, 60, 100), ev(Q, 0, 0x80, 60, 0), ev(0, 1, 0x90, 62, 100), ev(Q, 1, 0x80, 62, 0)];
        let inputs = midi_record_inputs(&e);
        for merge in [false, true] {
            let take =
                add_midi_recording_with(&mut e, LOOP, MidiPass::Linear, &events, &devices, &inputs, &style(false, &[merge], MergeRelease::default()));
            assert_eq!(take.clips.len() + take.merged.len(), 2, "one clip on each track (merge: {merge})");
        }
    }

    #[test]
    fn a_loop_round_stopped_at_its_wrap_does_not_latch_over_the_loop_start() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        // Round 1 moves controller 1 at the loop start and half way; the pass stops at the wrap.
        let events = [ev(0, 0, 0xB0, 1, 5), ev(2 * Q, 0, 0xB0, 1, 20), WRAP];
        record(&mut e, LOOP, MidiPass::Loop { stopped: Some(0), marked: true }, &events, &style(false, &[true], MergeRelease::Latch));
        let clip = e.session().track(t).unwrap().clips()[0].clone();
        let cc1: Vec<(i64, i32)> = midi_of(&clip).ctrls.iter().filter(|c| c.kind == CtrlKind::Cc(1)).map(|c| (c.tick, c.value)).collect();
        assert_eq!(cc1, [(0, 5), (1920, 20)]);
    }

    #[test]
    fn a_clip_whose_controllers_give_way_counts_as_merged_into() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        // Two clips side by side, each with a controller 1 value.
        add_midi_recording(&mut e, Range::new(0, Q), &[ev(Q / 2, 0, 0xB0, 1, 40)], &[], false);
        add_midi_recording(&mut e, Range::new(Q, 2 * Q), &[ev(Q + Q / 2, 0, 0xB0, 1, 50)], &[], false);
        let ids: Vec<ClipId> = e.session().track(t).unwrap().clips().iter().map(|c| c.id).collect();
        // Latch: one new value in the first clip writes over the second clip's to the stop.
        let take = record(&mut e, Range::new(0, 2 * Q), MidiPass::Linear, &[ev(Q / 4, 0, 0xB0, 1, 9)], &style(false, &[true], MergeRelease::Latch));
        assert!(midi_of(&e.session().track(t).unwrap().clips()[1]).ctrls.is_empty());
        let mut merged = take.merged.clone();
        merged.sort_unstable();
        assert_eq!(merged, ids);
    }

    #[test]
    fn a_merged_pass_of_one_tick_keeps_its_controller() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let take = record(&mut e, Range::new(0, 1), MidiPass::Linear, &[ev(0, 0, 0xB0, 1, 9)], &style(false, &[true], MergeRelease::default()));
        assert_eq!(take.clips.len(), 1);
        assert_eq!(midi_of(&e.session().track(t).unwrap().clips()[0]).ctrls.len(), 1);
    }

    #[test]
    fn a_clip_shorter_than_a_tick_still_receives_what_is_merged_into_it() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let merge = style(false, &[true], MergeRelease::default());
        record(&mut e, Range::new(0, 1), MidiPass::Linear, &[ev(0, 0, 0xB0, 1, 9)], &merge);
        let take = record(&mut e, Range::new(0, 1), MidiPass::Linear, &[ev(0, 0, 0xB0, 1, 30)], &merge);
        assert!(take.clips.is_empty());
        let clips = e.session().track(t).unwrap().clips();
        assert_eq!(clips.len(), 1);
        assert_eq!(midi_of(&clips[0]).ctrls.iter().map(|c| c.value).collect::<Vec<_>>(), [30]);
    }

    #[test]
    fn a_note_played_at_the_start_of_a_clip_shorter_than_a_tick_goes_where_it_can_sound_whole() {
        // Playback ends a note at its clip's end: in a clip shorter than a tick, a note would
        // hardly sound. The controller stays in that clip; the note goes into the gap after it.
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let merge = style(false, &[true], MergeRelease::default());
        record(&mut e, Range::new(0, 1), MidiPass::Linear, &[ev(0, 0, 0xB0, 1, 9)], &merge);
        let events = [ev(0, 0, 0xB0, 1, 30), ev(0, 0, 0x90, 60, 100), ev(Q / 2, 0, 0x80, 60, 0)];
        record(&mut e, Range::new(0, Q), MidiPass::Linear, &events, &merge);
        let clips: Vec<(Samples, Samples, Vec<(u8, i64, i64)>, Vec<i32>)> = e
            .session()
            .track(t)
            .unwrap()
            .clips()
            .iter()
            .map(|c| {
                let m = midi_of(c);
                (c.start, c.range().end, m.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect(), m.ctrls.iter().map(|c| c.value).collect())
            })
            .collect();
        assert_eq!(clips, [(0, 1, vec![], vec![30]), (1, Q, vec![(60, 0, 480)], vec![])]);
    }

    #[test]
    fn a_replacing_pass_punched_into_a_playing_loop_leaves_what_lies_before_the_punch_in() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        add_midi_recording(
            &mut e,
            LOOP,
            &[ev(Q / 4, 0, 0x90, 50, 100), ev(Q / 2, 0, 0x80, 50, 0), ev(3 * Q, 0, 0x90, 72, 100), ev(3 * Q + Q / 4, 0, 0x80, 72, 0)],
            &[],
            false,
        );
        // Record pressed at the third quarter of the playing loop; stopped in that round.
        let events = [ev(2 * Q + Q / 4, 0, 0x90, 74, 100), ev(2 * Q + Q / 2, 0, 0x80, 74, 0)];
        let stop = 2 * Q + 3 * Q / 4;
        let style = MidiRecordStyle { punched_in: Some(2 * Q), ..style(false, &[false], MergeRelease::default()) };
        record(&mut e, LOOP, MidiPass::Loop { stopped: Some(stop), marked: true }, &events, &style);
        let track = e.session().track(t).unwrap();
        let clips: Vec<(Samples, Samples, Vec<(u8, i64)>)> =
            track.clips().iter().map(|c| (c.start, c.range().end, midi_of(c).notes.iter().map(|n| (n.pitch, n.start)).collect())).collect();
        assert_eq!(clips, [(0, 2 * Q, vec![(50, 240)]), (2 * Q, stop, vec![(74, 240)]), (stop, LOOP.end, vec![(72, 240)])]);
        // A later round played the whole loop over: it replaces all of it.
        e.run("edit.undo").unwrap();
        let events =
            [ev(2 * Q + Q / 4, 0, 0x90, 74, 100), ev(2 * Q + Q / 2, 0, 0x80, 74, 0), WRAP, ev(Q / 4, 0, 0x90, 76, 100), ev(Q / 2, 0, 0x80, 76, 0)];
        record(&mut e, LOOP, MidiPass::Loop { stopped: None, marked: true }, &events, &style);
        assert_eq!(pitches(&e, t), [vec![76]]);
    }

    #[test]
    fn a_punched_in_round_keeps_what_it_recorded_before_the_punch_in_point_the_ui_saw() {
        // The UI read the playhead a moment before the audio engine began the pass (here: before
        // the loop went round on it): what the round recorded lies before that point.
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let events = [ev(Q / 4, 0, 0x90, 74, 100), ev(Q / 2, 0, 0x80, 74, 0)];
        let style = MidiRecordStyle { punched_in: Some(3 * Q), ..style(false, &[false], MergeRelease::default()) };
        record(&mut e, LOOP, MidiPass::Loop { stopped: Some(Q), marked: true }, &events, &style);
        let track = e.session().track(t).unwrap();
        let clips: Vec<(Samples, Samples, Vec<(u8, i64)>)> =
            track.clips().iter().map(|c| (c.start, c.range().end, midi_of(c).notes.iter().map(|n| (n.pitch, n.start)).collect())).collect();
        assert_eq!(clips, [(Q / 4, Q, vec![(74, 0)])]);
    }
}
