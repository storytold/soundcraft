use serde::{Deserialize, Serialize};

/// A MIDI note. Times are in model ticks ([`crate::PPQ`] per quarter note).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// MIDI pitch 0..=127 (60 = middle C, shown as "C3").
    pub pitch: u8,
    /// Note-on velocity 1..=127.
    pub velocity: u8,
    /// Note-off (release) velocity 0..=127; 64 when the source had none.
    pub release_velocity: u8,
    /// MIDI channel 0..=15.
    pub channel: u8,
    /// Start in ticks.
    pub start: i64,
    /// Length in ticks; always > 0.
    pub length: i64,
}

impl Note {
    /// Tick at which the note ends (`start + length`, saturating).
    pub fn end(&self) -> i64 {
        self.start.saturating_add(self.length)
    }
}

/// Kind of a continuous / channel controller event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CtrlKind {
    /// Control change with the controller number 0..=127. Value 0..=127.
    Cc(u8),
    /// Pitch bend. Value is centred: -8192..=8191 (0 = no bend).
    PitchBend,
    /// Channel (mono) aftertouch. Value 0..=127.
    ChannelPressure,
    /// Polyphonic aftertouch for the given pitch. Value 0..=127.
    PolyPressure(u8),
    /// Program change. Value is the program number 0..=127.
    Program,
}

/// A controller / channel event at a tick.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CtrlEvent {
    pub tick: i64,
    pub channel: u8,
    pub kind: CtrlKind,
    pub value: i32,
}

/// The MIDI content of a region or track.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    /// Notes, sorted by `(start, pitch)` after [`Sequence::sort`].
    pub notes: Vec<Note>,
    /// Controller events, sorted by tick after [`Sequence::sort`].
    pub ctrls: Vec<CtrlEvent>,
}

fn kind_key(k: CtrlKind) -> (u8, u8) {
    match k {
        CtrlKind::Cc(n) => (0, n),
        CtrlKind::PitchBend => (1, 0),
        CtrlKind::ChannelPressure => (2, 0),
        CtrlKind::PolyPressure(p) => (3, p),
        CtrlKind::Program => (4, 0),
    }
}

impl Sequence {
    /// Sorts notes by `(start, pitch, channel, length, velocity)` and controller events by tick
    /// (stable, so same-tick events keep their order).
    pub fn sort(&mut self) {
        self.notes.sort_by_key(|a| (a.start, a.pitch, a.channel, a.length, a.velocity));
        self.ctrls.sort_by_key(|c| c.tick);
    }

    /// The last tick covered by any note end or controller event (0 when empty).
    pub fn end_tick(&self) -> i64 {
        let n = self.notes.iter().map(Note::end).max();
        let c = self.ctrls.iter().map(|c| c.tick).max();
        match (n, c) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => 0,
        }
    }

    /// Notes whose start lies in the half-open range `[start, end)`.
    pub fn notes_in(&self, start: i64, end: i64) -> impl Iterator<Item = &Note> + '_ {
        self.notes.iter().filter(move |n| n.start >= start && n.start < end)
    }

    /// Notes that sound anywhere inside the half-open range `[start, end)`.
    pub fn notes_overlapping(&self, start: i64, end: i64) -> impl Iterator<Item = &Note> + '_ {
        self.notes.iter().filter(move |n| n.start < end && n.end() > start)
    }

    /// Removes duplicate notes (same channel, pitch and start; the longest one is kept) and
    /// duplicate controller events (same tick, channel and kind; the last one is kept, matching
    /// what a synth would end up with). Returns how many events were removed. The sequence is
    /// left sorted.
    pub fn remove_duplicates(&mut self) -> usize {
        let before = self.notes.len() + self.ctrls.len();
        // Longest first within each (start, pitch, channel) group, then keep the first.
        self.notes.sort_by(|a, b| (a.start, a.pitch, a.channel, b.length).cmp(&(b.start, b.pitch, b.channel, a.length)));
        self.notes.dedup_by(|later, kept| later.start == kept.start && later.pitch == kept.pitch && later.channel == kept.channel);

        self.ctrls.sort_by_key(|c| c.tick);
        let mut kept: Vec<CtrlEvent> = Vec::with_capacity(self.ctrls.len());
        for c in self.ctrls.drain(..) {
            // Within a tick, replace an earlier event of the same channel+kind.
            // Only events at the same tick (the tail of `kept`) can match.
            let slot =
                kept.iter_mut().rev().take_while(|k| k.tick == c.tick).find(|k| k.channel == c.channel && kind_key(k.kind) == kind_key(c.kind));
            match slot {
                Some(slot) => *slot = c,
                None => kept.push(c),
            }
        }
        self.ctrls = kept;
        self.sort();
        before - (self.notes.len() + self.ctrls.len())
    }
}
