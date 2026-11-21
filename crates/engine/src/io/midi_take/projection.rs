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
                let mut notes: Vec<_> = seq.notes.iter().map(|&n| n.retime(convert(n.start, true), convert(n.end(), false))).collect();
                let mut ctrls: Vec<_> = seq.ctrls.iter().map(|&c| c.retime(convert(c.tick, true), 0)).collect();
                if seq == score {
                    // Rounding on the two grids can change the sorted order of two events.
                    // Match recorded attacks by their MIDI attributes before taking the exact time.
                    exact_recording_times(&mut notes, &played.notes);
                    exact_recording_times(&mut ctrls, &played.ctrls);
                }
                (notes.into_iter().map(Some).collect(), ctrls.into_iter().map(Some).collect())
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

fn exact_recording_times<T: MidiEvent>(input: &mut [T], played: &[T]) {
    let mut by_voice = HashMap::<_, Vec<T>>::new();
    for &p in played.iter().rev() {
        let (_, value, release) = p.detail();
        by_voice.entry((p.channel(), p.kind(), value, release)).or_default().push(p);
    }
    for event in input {
        let (_, value, release) = event.detail();
        if let Some(p) = by_voice.get_mut(&(event.channel(), event.kind(), value, release)).and_then(Vec::pop) {
            *event = event.retime(p.tick(), p.end());
        }
    }
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
