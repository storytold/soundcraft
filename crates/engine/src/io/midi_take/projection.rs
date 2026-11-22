//! Projection between the editable score and external-clock playback.
use super::{CtrlEvent, CtrlKind, Note, Range, SampleRate, Sequence, Session, TempoMap};
use crate::{ClockTake, edit::TimeCtx};
use std::{collections::HashMap, sync::Arc};

type NoteRows = Vec<(Note, Option<Note>)>;
type ControlRows = Vec<(CtrlEvent, Option<CtrlEvent>)>;

#[derive(Debug, PartialEq)]
enum Events {
    Recorded { score: Sequence, played: Sequence },
    Edited { notes: NoteRows, ctrls: ControlRows },
}

#[derive(Debug, PartialEq)]
pub(crate) struct ClockTakeProjection {
    events: Events,
    score_grid: TempoMap,
    played_grid: TempoMap,
    sr: SampleRate,
    range: Range,
}

impl ClockTake {
    pub(crate) fn captured_range(&self) -> Range {
        self.capture.range
    }

    /// Store both timings together before sorting. A composed take owns only its
    /// own events; it never references earlier clips, takes, or undo history.
    pub(crate) fn from_events(t: TimeCtx<'_>, grid: &TempoMap, range: Range, mut notes: NoteRows, mut ctrls: ControlRows) -> (Sequence, Self) {
        notes.sort_by_key(|(n, _)| (n.start, n.pitch, n.channel, n.length, n.velocity));
        ctrls.sort_by_key(|(c, _)| c.tick);
        let score = Sequence { notes: notes.iter().map(|r| r.0).collect(), ctrls: ctrls.iter().map(|r| r.0).collect() };
        let projection =
            ClockTakeProjection { events: Events::Edited { notes, ctrls }, score_grid: t.tempo.clone(), played_grid: grid.clone(), sr: t.sr, range };
        (score, Self { capture: Arc::new(projection) })
    }

    /// Playback timings in input order so coincident events from different
    /// sources stay paired through consolidation, pitch splits and copy/paste.
    pub(crate) fn project_events(&self, seq: &Sequence) -> (Vec<Option<Note>>, Vec<Option<CtrlEvent>>) {
        self.capture.project_events(seq)
    }

    /// The editor knows which row the user deleted even if several editable
    /// notes are identical. Apply those indices before the remaining rows shift.
    pub(crate) fn remove_notes(&mut self, seq: &Sequence, deleted: &std::collections::HashSet<usize>) {
        let (notes, ctrls) = self.project_events(seq);
        let notes = seq.notes.iter().copied().zip(notes).enumerate().filter_map(|(i, row)| (!deleted.contains(&i)).then_some(row)).collect();
        let ctrls = seq.ctrls.iter().copied().zip(ctrls).collect();
        self.replace_rows(notes, ctrls);
    }

    /// Sequence::remove_duplicates keeps the first of equally long notes and
    /// the final controller at the same tick, channel and kind. Identical
    /// editable values in these groups may have distinct capture timings.
    pub(crate) fn remove_duplicates(&mut self, seq: &mut Sequence) -> usize {
        let (notes, ctrls) = self.project_events(seq);
        let note_key = |n: &Note| (n.start, n.channel, n.pitch, n.length);
        let ctrl_key = |c: &CtrlEvent| (c.tick, c.channel, c.kind());
        let mut first = HashMap::new();
        for (n, played) in seq.notes.iter().zip(notes) {
            first.entry(note_key(n)).or_insert(played);
        }
        let last: HashMap<_, _> = seq.ctrls.iter().zip(ctrls).map(|(c, played)| (ctrl_key(c), played)).collect();
        let removed = seq.remove_duplicates();
        if removed == 0 {
            return 0;
        }
        let notes = seq.notes.iter().map(|n| (*n, first[&note_key(n)])).collect();
        let ctrls = seq.ctrls.iter().map(|c| (*c, last[&ctrl_key(c)])).collect();
        self.replace_rows(notes, ctrls);
        removed
    }

    fn replace_rows(&mut self, notes: NoteRows, ctrls: ControlRows) {
        let source = &*self.capture;
        self.capture = Arc::new(ClockTakeProjection {
            events: Events::Edited { notes, ctrls },
            score_grid: source.score_grid.clone(),
            played_grid: source.played_grid.clone(),
            sr: source.sr,
            range: source.range,
        });
    }
}

impl ClockTakeProjection {
    pub(super) fn recorded(score: Sequence, played: Sequence, doc: &Session, grid: &TempoMap, range: Range) -> Self {
        Self { events: Events::Recorded { score, played }, score_grid: doc.tempo.clone(), played_grid: grid.clone(), sr: doc.sample_rate, range }
    }

    /// The exact two capture passes include the recorder's half-open boundary rounding.
    pub(crate) fn project(&self, seq: &mut Sequence) {
        if let Events::Recorded { score, played } = &self.events
            && seq == score
        {
            seq.clone_from(played);
            return;
        }
        let (notes, ctrls) = self.project_events(seq);
        seq.notes = notes.into_iter().flatten().collect();
        seq.ctrls = ctrls.into_iter().flatten().collect();
        seq.sort();
    }

    fn project_events(&self, seq: &Sequence) -> (Vec<Option<Note>>, Vec<Option<CtrlEvent>>) {
        let score_start = self.score_grid.samples_to_ticks(self.range.start, self.sr);
        let play_start = self.played_grid.samples_to_ticks(self.range.start, self.sr);
        let score_end = self.score_grid.samples_to_ticks(self.range.end, self.sr) - score_start;
        let play_end = self.played_grid.samples_to_ticks(self.range.end, self.sr) - play_start;
        let convert = |tick: i64, onset: bool| {
            let sample = self.score_grid.tick_to_samples(score_start.saturating_add(tick), self.sr);
            let out = self.played_grid.samples_to_ticks(sample, self.sr).saturating_sub(play_start).max(0);
            if onset && tick < score_end { out.min(play_end.saturating_sub(1).max(0)) } else { out }
        };
        match &self.events {
            Events::Edited { notes, ctrls } => (project_edited(&seq.notes, notes, &convert), project_edited(&seq.ctrls, ctrls, &convert)),
            Events::Recorded { score, played } => {
                let notes = recorded_rows(&score.notes, &played.notes);
                let ctrls = recorded_rows(&score.ctrls, &played.ctrls);
                (project_edited(&seq.notes, &notes, &convert), project_edited(&seq.ctrls, &ctrls, &convert))
            }
        }
    }
}

/// The model stores editable notes/controllers by value. Reserve exact matches
/// before matching velocity, pitch or timing edits, so removing a sibling at the
/// same tick cannot steal the surviving event's timing.
trait MidiEvent: Copy + PartialEq {
    fn tick(self) -> i64;
    fn end(self) -> i64;
    fn channel(self) -> u8;
    fn kind(self) -> u16;
    fn detail(self) -> (i64, i32, i32);
    fn retime(self, start: i64, end: i64) -> Self;
}
impl MidiEvent for Note {
    fn tick(self) -> i64 {
        self.start
    }
    fn end(self) -> i64 {
        Note::end(&self)
    }
    fn channel(self) -> u8 {
        self.channel
    }
    fn kind(self) -> u16 {
        self.pitch.into()
    }
    fn detail(self) -> (i64, i32, i32) {
        (self.length, self.velocity.into(), self.release_velocity.into())
    }
    fn retime(self, start: i64, end: i64) -> Self {
        Self { start, length: end.saturating_sub(start).max(1), ..self }
    }
}
impl MidiEvent for CtrlEvent {
    fn tick(self) -> i64 {
        self.tick
    }
    fn end(self) -> i64 {
        self.tick
    }
    fn channel(self) -> u8 {
        self.channel
    }
    fn kind(self) -> u16 {
        match self.kind {
            CtrlKind::Cc(n) => n.into(),
            CtrlKind::PolyPressure(p) => 128 + u16::from(p),
            CtrlKind::PitchBend => 256,
            CtrlKind::Program => 257,
            CtrlKind::ChannelPressure => 258,
        }
    }
    fn detail(self) -> (i64, i32, i32) {
        (0, self.value, 0)
    }
    fn retime(self, start: i64, _: i64) -> Self {
        Self { tick: start, ..self }
    }
}

// Rounding on the two grids can reorder events. Pair the unedited capture
// passes first, so changing a velocity or CC value cannot affect that pairing.
fn recorded_rows<T: MidiEvent>(score: &[T], played: &[T]) -> Vec<(T, Option<T>)> {
    let mut by_voice = HashMap::<_, Vec<T>>::new();
    for &p in played.iter().rev() {
        let (_, value, release) = p.detail();
        by_voice.entry((p.channel(), p.kind(), value, release)).or_default().push(p);
    }
    score
        .iter()
        .map(|&event| {
            let (_, value, release) = event.detail();
            let played = by_voice.get_mut(&(event.channel(), event.kind(), value, release)).and_then(Vec::pop);
            (event, played)
        })
        .collect()
}

fn project_edited<T: MidiEvent>(input: &[T], rows: &[(T, Option<T>)], convert: &impl Fn(i64, bool) -> i64) -> Vec<Option<T>> {
    if input.len() == rows.len() && input.iter().zip(rows).all(|(e, (old, _))| e == old) {
        return rows.iter().map(|r| r.1).collect();
    }
    let mut matches = vec![None; input.len()];
    let mut used = vec![false; rows.len()];
    let mut remaining = input.len();
    // A typical edit changes one event. Unchanged rows at their existing
    // indices need no lookup tables, even in a large consolidated performance.
    for (i, (event, (base, _))) in input.iter().zip(rows).enumerate() {
        if event == base {
            matches[i] = Some(i);
            used[i] = true;
            remaining -= 1;
        }
    }
    for pass in 0..4 {
        if remaining == 0 {
            break;
        }
        let key = |e: T| {
            let (length, value, release) = e.detail();
            match pass {
                0 => (e.tick(), e.kind(), e.channel(), length, value, release),
                1 => (e.tick(), e.kind(), e.channel(), 0, 0, 0),
                2 => (e.tick(), 0, e.channel(), length, value, release),
                _ => (0, e.kind(), e.channel(), length, value, release),
            }
        };
        let mut available = HashMap::<_, Vec<usize>>::new();
        for (i, &(event, _)) in rows.iter().enumerate().rev().filter(|(i, _)| !used[*i]) {
            available.entry(key(event)).or_default().push(i);
        }
        for (idx, &event) in input.iter().enumerate() {
            if matches[idx].is_none()
                && let Some(i) = available.get_mut(&key(event)).and_then(Vec::pop)
            {
                matches[idx] = Some(i);
                used[i] = true;
                remaining -= 1;
            }
        }
    }
    input
        .iter()
        .zip(matches)
        .map(|(&event, index)| {
            let Some(i) = index else { return Some(event.retime(convert(event.tick(), true), convert(event.end(), false))) };
            let (old, played) = rows[i];
            let played = played?;
            // An unchanged endpoint retains the exact captured tick. Moving an endpoint
            // transfers only the edit's delta, measured on this clip's score and follow grids.
            let delta = |now, old, attack| if now == old { 0 } else { convert(now, attack).saturating_sub(convert(old, attack)) };
            let start = played.tick().saturating_add(delta(event.tick(), old.tick(), true)).max(0);
            let end = played.end().saturating_add(delta(event.end(), old.end(), false));
            Some(event.retime(start, end))
        })
        .collect()
}
