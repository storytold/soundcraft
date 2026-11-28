//! MIDI Merge: what a record pass played, written into the MIDI already on a track.
//!
//! Pro Tools Reference Guide: "When MIDI Merge is on (Merge mode), recorded MIDI is merged with
//! existing track material" (p.813); "existing MIDI clips are never overwritten … the existing
//! clip boundaries remain. New MIDI clips are only created to fill the gaps between existing MIDI
//! clip boundaries" (p.819); MIDI Merge Release mode (p.814, p.176) for controllers.

use super::{Clip, MergeRelease, Range, Samples, Session, TrackId};
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{ClipContent, ClipId};
use std::collections::HashMap;

/// A controller of one channel: what MIDI Merge Release mode applies to. `None` for the kinds it
/// does not: program changes and polyphonic aftertouch, which are merged as they are. Pro Tools
/// lists volume, pan, pitch bend, mono aftertouch and controllers 0–127 as its continuous
/// controller events (Reference Guide p.1042–1043).
fn controller(c: &CtrlEvent) -> Option<(u8, u8, u8)> {
    match c.kind {
        CtrlKind::Cc(n) => Some((c.channel, 0, n)),
        CtrlKind::PitchBend => Some((c.channel, 1, 0)),
        CtrlKind::ChannelPressure => Some((c.channel, 2, 0)),
        CtrlKind::PolyPressure(_) | CtrlKind::Program => None,
    }
}

/// The release mode of a pass and, for Latch, the value each controller was left at by the
/// rounds merged so far: Latch goes on writing it over the later rounds of a loop until the pass
/// stops.
pub(super) struct Latch {
    release: MergeRelease,
    last: HashMap<(u8, u8, u8), CtrlEvent>,
}

impl Latch {
    pub(super) fn new(release: MergeRelease) -> Self {
        Self { release, last: HashMap::new() }
    }
}

/// A clip on the track that the pass merges into.
struct Host {
    id: ClipId,
    /// Its start and end in ticks of the timeline.
    from: i64,
    to: i64,
    span: Range,
    /// Shorter than a tick (`to` was made a tick past `from`): a note in it would end at its
    /// start on playback.
    short: bool,
}

/// Merge `seq` (in ticks from the start of `range`) into the MIDI clips of `track`'s playlist that
/// the range covers, and fill the parts of the range that no such clip covers with new clips
/// (named `name`) that hold what was played there. `until` is where the pass stopped, for Latch.
/// Returns the clips made and the clips merged into.
///
/// - Each note goes into the clip its start is in (the later-starting clip where they overlap: the
///   one on top); each controller into the clip its tick is in. Clip boundaries do not change. A
///   clip shorter than a tick, which would end such a note at its start, takes a note only when no
///   gap does (a controller there stays in it). A
///   note is stored whole even when it runs past the end of its clip (into a clip after it):
///   Pro Tools allows notes to "extend beyond the clip boundary" (p.819), but playback and MIDI
///   export end every note at its clip's end today, so its tail is not heard.
/// - An edit-locked clip is not merged into: "Clips that are Edit-locked cannot be edited"
///   (p.1237). What was played over it goes into a new clip laid over it, as a replacing take
///   does.
/// - A note played over a note of the same key and channel on the track: the earlier of the two
///   ends where the later starts (a key cannot sound twice); two that start together are one
///   note, as long as the longer of them.
/// - Controllers as the release mode says ([`MergeRelease`]).
pub(super) fn merge_take(
    s: &mut Session,
    track: TrackId,
    name: &str,
    range: Range,
    until: Samples,
    seq: &Sequence,
    latch: &mut Latch,
) -> (Vec<ClipId>, Vec<ClipId>) {
    let sr = s.sample_rate;
    let tempo = s.tempo.clone();
    let ticks = |at: Samples| tempo.samples_to_ticks(at, sr);
    let base = ticks(range.start);
    // The end of the range as the take was made: at least a tick past its start, so that what a
    // pass shorter than a tick took in at its start is kept.
    let range_to = ticks(range.end).max(base.saturating_add(1));
    let stop = ticks(until.clamp(range.start, range.end));
    // What was played, on the timeline's ticks.
    let mut notes: Vec<Note> = seq.notes.iter().map(|n| Note { start: n.start.saturating_add(base), ..*n }).collect();
    let mut ctrls: Vec<CtrlEvent> = seq.ctrls.iter().map(|c| CtrlEvent { tick: c.tick.saturating_add(base), ..*c }).collect();
    // Latch: a controller latched in an earlier round goes on with its last value from the start
    // of this one, up to its first new value.
    if latch.release == MergeRelease::Latch {
        for (key, last) in &latch.last {
            let first = ctrls.iter().filter(|c| controller(c) == Some(*key)).map(|c| c.tick).min();
            if first != Some(base) {
                ctrls.push(CtrlEvent { tick: base, ..*last });
            }
        }
        ctrls.sort_by_key(|c| c.tick);
    }
    // The span of each controller that the existing events give way to.
    let mut spans: HashMap<(u8, u8, u8), (i64, i64)> = HashMap::new();
    for c in &ctrls {
        if let Some(key) = controller(c) {
            let e = spans.entry(key).or_insert((c.tick, c.tick));
            e.0 = e.0.min(c.tick);
            e.1 = e.1.max(c.tick);
        }
    }
    for (key, span) in &mut spans {
        if latch.release == MergeRelease::Latch {
            // To the stop: the end tick itself is outside the pass.
            span.1 = stop.max(span.1.saturating_add(1)).saturating_sub(1);
            if let Some(last) = ctrls.iter().rev().find(|c| controller(c) == Some(*key)) {
                latch.last.insert(*key, *last);
            }
        }
    }
    let hosts: Vec<Host> = s
        .track(track)
        .map(|t| {
            t.clips()
                .iter()
                .filter(|c| matches!(c.content, ClipContent::Midi { .. }) && !c.edit_locked && c.start < range.end && c.end() > range.start)
                .map(|c| {
                    // At least a tick long, as the range is: a clip shorter than a tick still
                    // takes what is played at its start.
                    let (from, to) = (ticks(c.start), ticks(c.end()));
                    Host { id: c.id, from, to: to.max(from.saturating_add(1)), span: c.range(), short: to <= from }
                })
                .collect()
        })
        .unwrap_or_default();
    // The clips merged into: those that received something and those whose own notes or
    // controllers gave way to it.
    let mut merged: Vec<ClipId> = Vec::new();
    // Existing notes of the same key give way to the new ones, in every clip merged into.
    for h in &hosts {
        let Some(hs) = host_sequence(s, h.id) else { continue };
        let mut gave_way = false;
        let mut kept = Vec::with_capacity(hs.notes.len());
        for mut x in hs.notes.drain(..) {
            x.start = x.start.saturating_add(h.from);
            let mut keep = true;
            for n in &mut notes {
                if n.channel != x.channel || n.pitch != x.pitch || n.start >= x.end() || n.end() <= x.start {
                    continue;
                }
                if x.start == n.start {
                    // Two that start together are one: the longer stays, as when a sequence's
                    // duplicates are removed.
                    if x.length > n.length {
                        n.length = x.length;
                    }
                    keep = false;
                    gave_way = true;
                } else if x.start < n.start {
                    x.length = n.start - x.start;
                    gave_way = true;
                } else {
                    n.length = x.start - n.start;
                }
            }
            if keep {
                x.start = x.start.saturating_sub(h.from);
                kept.push(x);
            }
        }
        hs.notes = kept;
        let ctrls_before = hs.ctrls.len();
        hs.ctrls.retain(|c| {
            let at = c.tick.saturating_add(h.from);
            !controller(c).and_then(|key| spans.get(&key)).is_some_and(|&(a, b)| at >= a && at <= b)
        });
        if gave_way || hs.ctrls.len() != ctrls_before {
            note_merged(&mut merged, h.id);
        }
    }
    // The parts of the range no clip merged into covers, and their ticks: what is not merged
    // goes into new clips there.
    let gap_ticks: Vec<(Range, i64, i64)> = gaps(range, hosts.iter().map(|h| h.span))
        .into_iter()
        .map(|gap| (gap, ticks(gap.start), if gap.end == range.end { range_to } else { ticks(gap.end) }))
        .collect();
    let in_gap = |tick: i64| gap_ticks.iter().any(|&(_, from, to)| tick >= from && tick < to);
    // Which clip each event goes into: the latest-starting one that holds its tick. Playback ends
    // a note at its clip's end, so a clip shorter than a tick takes a note only when no gap can.
    let host_of = |tick: i64, note: bool| {
        hosts
            .iter()
            .filter(|h| h.from <= tick && tick < h.to && !(note && h.short && in_gap(tick)))
            .max_by_key(|h| (h.from, h.span.start))
            .map(|h| h.id)
    };
    let mut loose = Sequence::default();
    for n in notes {
        match host_of(n.start, true) {
            Some(id) => {
                if let Some((hs, from)) = host_sequence(s, id).zip(hosts.iter().find(|h| h.id == id).map(|h| h.from)) {
                    hs.notes.push(Note { start: n.start - from, ..n });
                    note_merged(&mut merged, id);
                }
            }
            None => loose.notes.push(n),
        }
    }
    for c in ctrls {
        match host_of(c.tick, false) {
            Some(id) => {
                if let Some((hs, from)) = host_sequence(s, id).zip(hosts.iter().find(|h| h.id == id).map(|h| h.from)) {
                    hs.ctrls.push(CtrlEvent { tick: c.tick - from, ..c });
                    note_merged(&mut merged, id);
                }
            }
            None => loose.ctrls.push(c),
        }
    }
    for h in &hosts {
        if let Some(hs) = host_sequence(s, h.id) {
            hs.sort();
        }
    }
    // The rest goes into new clips over the gaps.
    let mut made = Vec::new();
    for &(gap, from, to) in &gap_ticks {
        let inside = |t: i64| t >= from && t < to;
        let mut piece = Sequence::default();
        piece.notes.extend(loose.notes.iter().filter(|n| inside(n.start)).map(|n| Note { start: n.start - from, ..*n }));
        piece.ctrls.extend(loose.ctrls.iter().filter(|c| inside(c.tick)).map(|c| CtrlEvent { tick: c.tick - from, ..*c }));
        if piece.notes.is_empty() && piece.ctrls.is_empty() {
            continue;
        }
        piece.sort();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, track, Clip::midi(cid, name, gap.start, gap.len(), piece));
        made.push(cid);
    }
    (made, merged)
}

fn note_merged(merged: &mut Vec<ClipId>, id: ClipId) {
    if !merged.contains(&id) {
        merged.push(id);
    }
}

/// The sequence of the MIDI clip `id`.
fn host_sequence(s: &mut Session, id: ClipId) -> Option<&mut Sequence> {
    match &mut s.find_clip_mut(id)?.content {
        ClipContent::Midi { sequence } => Some(sequence),
        _ => None,
    }
}

/// The parts of `range` that none of `covered` covers, in order.
fn gaps(range: Range, covered: impl Iterator<Item = Range>) -> Vec<Range> {
    let mut covered: Vec<Range> = covered.filter_map(|c| c.intersect(&range)).collect();
    covered.sort_by_key(|c| c.start);
    let mut out = Vec::new();
    let mut at = range.start;
    for c in covered {
        if c.start > at {
            out.push(Range::new(at, c.start));
        }
        at = at.max(c.end);
    }
    if at < range.end {
        out.push(Range::new(at, range.end));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::gaps;
    use crate::io::Range;

    #[test]
    fn the_gaps_are_what_no_clip_covers() {
        let r = Range::new(0, 100);
        assert_eq!(gaps(r, [Range::new(10, 20), Range::new(15, 30), Range::new(90, 200)].into_iter()), [Range::new(0, 10), Range::new(30, 90)]);
        assert_eq!(gaps(r, std::iter::empty()), [r]);
        assert!(gaps(r, [Range::new(-5, 105)].into_iter()).is_empty());
    }
}
