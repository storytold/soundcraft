//! Editing algorithms on playlists, independent of the command layer.
//!
//! Model rules: clips on a playlist are sorted and do not overlap, except where two clips
//! overlap exactly over a crossfade (the earlier clip fades out while the later fades in).
//! Placing a clip overwrites whatever is under it (like the incumbent's Slip mode); Shuffle
//! mode also ripples later clips.

use soundcraft_model::{AutoParam, AutomationPoint, Clip, ClipContent, ClipId, Fade, FadeShape, Session, Track, TrackId};
use soundcraft_time::{Range, SampleRate, Samples, TempoMap};
use std::collections::{HashMap, HashSet};

/// Edit-state flag: the Edit cursor and selections snap to the grid in Shuffle, Slip and Spot.
pub const SNAP_TO_GRID: &str = "edit.snap_to_grid";
/// Edit-state flag: Shuffle mode can't be chosen.
pub const SHUFFLE_LOCK: &str = "edit.shuffle_lock";
/// Edit-state flag: the Grid mode last used was Relative, so choosing Grid again picks it.
pub const RELATIVE_GRID: &str = "edit.relative_grid";

/// A timeline edit may be working on a standalone document or on the engine's followed takes.
pub struct ClipEdit<'a> {
    pub(crate) doc: &'a mut Session,
    pub(crate) takes: Option<&'a mut crate::ClockTakes>,
    pub(crate) follow_grid: Option<&'a TempoMap>,
}

impl std::ops::Deref for ClipEdit<'_> {
    type Target = Session;
    fn deref(&self) -> &Session {
        self.doc
    }
}
impl std::ops::DerefMut for ClipEdit<'_> {
    fn deref_mut(&mut self) -> &mut Session {
        self.doc
    }
}

/// Supplies both halves of a timeline edit when new clip identities can be allocated.
pub trait EditTimeline {
    fn clip_edit(&mut self) -> ClipEdit<'_>;
}
impl EditTimeline for Session {
    fn clip_edit(&mut self) -> ClipEdit<'_> {
        ClipEdit { doc: self, takes: None, follow_grid: None }
    }
}
impl EditTimeline for crate::Engine {
    fn clip_edit(&mut self) -> ClipEdit<'_> {
        crate::Engine::clip_edit(self)
    }
}
impl EditTimeline for ClipEdit<'_> {
    fn clip_edit(&mut self) -> ClipEdit<'_> {
        ClipEdit { doc: self.doc, takes: self.takes.as_deref_mut(), follow_grid: self.follow_grid }
    }
}

impl ClipEdit<'_> {
    /// Whole clips inherit their document provenance; a range copy supplies its own trimmed snapshot.
    pub(crate) fn assign_copied_id(&mut self, clip: &mut Clip, source_takes: Option<&crate::ClockTakes>) {
        let id = self.doc.new_clip_id();
        if let Some(takes) = self.takes.as_deref_mut()
            && let Some(take) = source_takes.unwrap_or(takes).get(&clip.id).cloned()
        {
            takes.insert(id, take);
        }
        clip.id = id;
    }
}

/// Context needed to split MIDI clips in musical time.
#[derive(Clone, Copy)]
pub struct TimeCtx<'a> {
    pub tempo: &'a TempoMap,
    pub sr: SampleRate,
}

impl<'a> TimeCtx<'a> {
    pub fn of(s: &'a Session) -> Self {
        TimeCtx { tempo: &s.tempo, sr: s.sample_rate }
    }
    fn ticks(&self, s: Samples) -> i64 {
        self.tempo.samples_to_ticks(s, self.sr)
    }
}

/// Split `clip` at absolute position `at` (strictly inside). Returns (left, right) where right gets `new_id`.
pub fn split_clip(clip: &Clip, at: Samples, new_id: ClipId, t: TimeCtx<'_>) -> Option<(Clip, Clip)> {
    split_clip_with_take(clip, at, new_id, t, None, None)
}

fn split_clip_with_take(
    clip: &Clip,
    at: Samples,
    new_id: ClipId,
    t: TimeCtx<'_>,
    grid: Option<&TempoMap>,
    takes: Option<&mut crate::ClockTakes>,
) -> Option<(Clip, Clip)> {
    if at <= clip.start || at >= clip.end() {
        return None;
    }
    let mut left = clip.clone();
    let mut right = clip.clone();
    right.id = new_id;
    left.trim_end_to(at);
    right.trim_start_to(at);
    left.fade_out = Fade::default();
    right.fade_in = Fade::default();
    if let Some(takes) = takes
        && let Some(grid) = grid
        && let Some(take) = takes.get(&clip.id)
        && let ClipContent::Midi { sequence } = &clip.content
    {
        let (ls, lt) = take.slice_events(sequence, clip.range(), left.range(), t, grid);
        let (rs, rt) = take.slice_events(sequence, clip.range(), right.range(), t, grid);
        left.content = ClipContent::Midi { sequence: ls };
        right.content = ClipContent::Midi { sequence: rs };
        takes.insert(left.id, lt);
        takes.insert(right.id, rt);
    } else if let (ClipContent::Midi { sequence: ls }, ClipContent::Midi { sequence: rs }) = (&mut left.content, &mut right.content) {
        let boundary = t.ticks(at).saturating_sub(t.ticks(clip.start));
        ls.notes.retain(|n| n.start < boundary);
        ls.ctrls.retain(|c| c.tick < boundary);
        rs.notes.retain(|n| n.start >= boundary);
        rs.ctrls.retain(|c| c.tick >= boundary);
        for n in &mut rs.notes {
            n.start -= boundary;
        }
        for c in &mut rs.ctrls {
            c.tick -= boundary;
        }
    }
    Some((left, right))
}

/// Trim a MIDI clip's start: drop notes before the new start and re-anchor the rest.
fn rebase_midi_start(clip: &mut Clip, old_start: Samples, t: TimeCtx<'_>) {
    let delta = t.ticks(clip.start).saturating_sub(t.ticks(old_start));
    if let ClipContent::Midi { sequence } = &mut clip.content {
        if delta == 0 {
            return;
        }
        sequence.notes.retain(|n| n.start >= delta);
        sequence.ctrls.retain(|c| c.tick >= delta);
        for n in &mut sequence.notes {
            n.start -= delta;
        }
        for c in &mut sequence.ctrls {
            c.tick -= delta;
        }
    }
}

/// Split every clip on a track's active playlist that spans `at`, except time- or edit-locked ones.
/// The piece before `at` keeps the clip's name. The piece after it is a new clip, and takes the next
/// name of the clip's Auto Rename scheme when it has one.
pub fn separate_at(timeline: &mut impl EditTimeline, track: TrackId, at: Samples) -> usize {
    separate_at_if(timeline, track, at, |_| true)
}

/// [`separate_at`], giving the piece after `at` the next Auto Rename name only when `auto_name`
/// returns true for it. Any other piece keeps the clip's name, and its scheme does not count up.
pub fn separate_at_if(timeline: &mut impl EditTimeline, track: TrackId, at: Samples, auto_name: impl Fn(&Clip) -> bool) -> usize {
    split_at(timeline, track, at, auto_name, true)
}

/// [`separate_at`], splitting time- and edit-locked clips too. For the Event time operations, which
/// move the whole timeline, locked clips included.
pub fn separate_all_at(timeline: &mut impl EditTimeline, track: TrackId, at: Samples) -> usize {
    split_at(timeline, track, at, |_| true, false)
}

fn split_at(timeline: &mut impl EditTimeline, track: TrackId, at: Samples, auto_name: impl Fn(&Clip) -> bool, skip_locked: bool) -> usize {
    let ClipEdit { doc: s, mut takes, follow_grid } = timeline.clip_edit();
    let tc_tempo = s.tempo.clone();
    let sr = s.sample_rate;
    let t = TimeCtx { tempo: &tc_tempo, sr };
    let mut ids = Vec::new();
    let n_split = s.track(track).map_or(0, |tr| tr.clips().iter().filter(|c| at > c.start && at < c.end() && !(skip_locked && locked(c))).count());
    for _ in 0..n_split {
        ids.push(s.new_clip_id());
    }
    let mut schemes = std::mem::take(&mut s.auto_names);
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else {
        s.auto_names = schemes;
        return 0;
    };
    let mut out = Vec::with_capacity(pl.clips.len() + ids.len());
    let mut count = 0;
    for c in pl.clips.drain(..) {
        if at > c.start
            && at < c.end()
            && !(skip_locked && locked(&c))
            && let Some(id) = ids.pop()
            && let Some((l, mut r)) = split_clip_with_take(&c, at, id, t, follow_grid, takes.as_deref_mut())
        {
            if auto_name(&r) {
                take_auto_name(&mut schemes, &mut r);
            }
            out.push(l);
            out.push(r);
            count += 1;
            continue;
        }
        out.push(c);
    }
    pl.clips = out;
    pl.sort();
    s.auto_names = schemes;
    count
}

/// Split every clip on a track's active playlist at each of `points`, in one pass over the
/// playlist: [`separate_at_if`] once per point with `|_| false` (no piece takes an Auto Rename
/// name here), without rebuilding the playlist per point. Returns the number of splits.
/// `points` are sorted and deduplicated first, so the source offsets of stretched audio can differ
/// from per-point calls given in another order (each trim rounds its own adjustment).
pub fn separate_at_points(timeline: &mut impl EditTimeline, track: TrackId, points: &[Samples]) -> usize {
    let ClipEdit { doc: s, mut takes, follow_grid } = timeline.clip_edit();
    let mut pts = points.to_vec();
    pts.sort_unstable();
    pts.dedup();
    let tc_tempo = s.tempo.clone();
    let t = TimeCtx { tempo: &tc_tempo, sr: s.sample_rate };
    // The points strictly inside a clip.
    let within = |c: &Clip| {
        let (lo, hi) = (pts.partition_point(|p| *p <= c.start), pts.partition_point(|p| *p < c.end()));
        lo..hi.max(lo)
    };
    let n_split: usize = s.track(track).map_or(0, |tr| tr.clips().iter().map(|c| within(c).len()).sum());
    let mut ids: Vec<ClipId> = (0..n_split).map(|_| s.new_clip_id()).collect();
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut out = Vec::with_capacity(pl.clips.len() + n_split);
    for c in pl.clips.drain(..) {
        let mut rest = c;
        for p in &pts[within(&rest)] {
            let Some(id) = ids.pop() else { break };
            match split_clip_with_take(&rest, *p, id, t, follow_grid, takes.as_deref_mut()) {
                Some((l, r)) => {
                    out.push(l);
                    rest = r;
                }
                None => break,
            }
        }
        out.push(rest);
    }
    pl.clips = out;
    pl.sort();
    n_split
}

/// Gives `c` the next name of its Auto Rename scheme, when it has one, and counts the scheme up.
fn take_auto_name(schemes: &mut std::collections::BTreeMap<u64, soundcraft_model::AutoName>, c: &mut Clip) {
    if let Some(scheme) = c.auto_name.and_then(|k| schemes.get_mut(&k)) {
        c.name = crate::io::StripNames::from(&*scheme).clip_name(0);
        scheme.number = scheme.number.saturating_add(1);
    }
}

/// Gives each clip on a track's active playlist whose id is not in `old`, and for which `auto_name`
/// returns true, the next name of its Auto Rename scheme, in time order. Pieces cut with
/// [`separate_at_if`] and `|_| false` take their names here once the caller has removed the ones it
/// does not keep.
pub fn auto_name_new_clips(s: &mut Session, track: TrackId, old: &HashSet<ClipId>, auto_name: impl Fn(&Clip) -> bool) {
    let mut schemes = std::mem::take(&mut s.auto_names);
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        let mut new: Vec<&mut Clip> = pl.clips.iter_mut().filter(|c| !old.contains(&c.id) && auto_name(c)).collect();
        new.sort_by_key(|c| (c.start, c.id.0));
        for c in new {
            take_auto_name(&mut schemes, c);
        }
    }
    s.auto_names = schemes;
}

/// Remove material inside `range` on a track. Time- and edit-locked clips stay whole: they are
/// neither split at the range's edges nor removed, so placing a clip over one overlaps it. In
/// Shuffle mode later clips move left, up to the first locked clip (see [`ripple`]); a locked clip
/// left inside the range holds everything after it in place. Returns the removed material as clips
/// positioned relative to `range.start`.
pub fn clear_range(timeline: &mut impl EditTimeline, track: TrackId, range: Range, shuffle: bool) -> Vec<Clip> {
    remove_range(timeline, track, range, shuffle, true)
}

/// [`clear_range`], splitting and removing time- and edit-locked clips too. For the Event time
/// operations, which cut the whole timeline, locked clips included.
pub fn clear_all_range(timeline: &mut impl EditTimeline, track: TrackId, range: Range, shuffle: bool) -> Vec<Clip> {
    remove_range(timeline, track, range, shuffle, false)
}

/// Whether a time- or edit-locked clip on `track` covers any of `range`.
pub fn holds_locked(s: &Session, track: TrackId, range: Range) -> bool {
    s.track(track).is_some_and(|t| t.clips().iter().any(|c| locked(c) && c.start < range.end && c.end() > range.start))
}

/// Why a consolidate is refused: it would replace a locked clip, whose material it has already
/// rendered, so keeping the clip whole would play it twice.
pub const LOCKED_IN_RANGE: &str = "the range holds a time- or edit-locked clip; unlock it first";

/// [`clear_range`] (`keep_locked`) or [`clear_all_range`].
fn remove_range(timeline: &mut impl EditTimeline, track: TrackId, range: Range, shuffle: bool, keep_locked: bool) -> Vec<Clip> {
    let mut s = timeline.clip_edit();
    if range.is_empty() {
        return Vec::new();
    }
    let separate = if keep_locked { separate_at } else { separate_all_at };
    separate(&mut s, track, range.start);
    separate(&mut s, track, range.end);
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return Vec::new() };
    let mut removed = Vec::new();
    pl.clips.retain(|c| {
        if c.start >= range.start && c.end() <= range.end && !(keep_locked && locked(c)) {
            let mut r = c.clone();
            r.start -= range.start;
            removed.push(r);
            false
        } else {
            true
        }
    });
    // Like Shuffle's ripple, a locked clip still in the range keeps the clips after it where they are.
    if shuffle && !holds_locked(&s, track, range) {
        let moved = ripple(&mut s, track, range.end, -range.len());
        follow_automation(&mut s, track, range.end, moved);
    }
    removed
}

/// Remove the clips `ids` from every track. In Shuffle the later clips on each track close the holes the movable
/// ones leave, as a Shuffle move lifts them (see [`lift_plan`]), so a crossfade partner keeps its material. Returns
/// how many clips went.
pub fn remove_clips(s: &mut Session, ids: &[ClipId], shuffle: bool) -> usize {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let mut n = 0;
    for track in s.tracks.iter().map(|t| t.id).collect::<Vec<_>>() {
        if shuffle {
            n += lift_all(s, track, &ids).len();
        }
        // Locked clips don't lift; they go without closing up, as everything does outside Shuffle.
        if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
            let before = pl.clips.len();
            pl.clips.retain(|c| !ids.contains(&c.id));
            n += before - pl.clips.len();
        }
    }
    n
}

/// Copy the material inside `range` (clips trimmed to the range), relative to `range.start`.
pub fn copy_range(s: &Session, track: TrackId, range: Range) -> Vec<Clip> {
    copy_range_with_takes(s, None, None, track, range, false).0
}

/// Copies score events and their capture origins together, before placing or moving anything.
pub(crate) fn copy_range_with_takes(
    s: &Session,
    takes: Option<&crate::ClockTakes>,
    grid: Option<&TempoMap>,
    track: TrackId,
    range: Range,
    unlocked_only: bool,
) -> (Vec<Clip>, crate::ClockTakes) {
    let t = TimeCtx::of(s);
    let mut captured = crate::ClockTakes::new();
    let Some(tr) = s.track(track) else { return (Vec::new(), captured) };
    let mut out = Vec::new();
    for c in tr.clips().iter().filter(|c| !unlocked_only || !locked(c)) {
        let Some(isect) = c.range().intersect(&range) else { continue };
        let take = takes.and_then(|m| m.get(&c.id));
        let mut part = c.clone();
        if let (Some(take), Some(grid), ClipContent::Midi { sequence }) = (take, grid, &c.content) {
            let child = if isect == c.range() {
                take.clone()
            } else {
                let (events, child) = take.slice_events(sequence, c.range(), isect, t, grid);
                part.content = ClipContent::Midi { sequence: events };
                child
            };
            captured.insert(c.id, child);
        }
        if isect.start > part.start {
            let old = part.start;
            part.trim_start_to(isect.start);
            if take.is_none() || grid.is_none() {
                rebase_midi_start(&mut part, old, t);
            }
            part.fade_in = Fade::default();
        }
        if isect.end < part.end() {
            part.trim_end_to(isect.end);
            part.fade_out = Fade::default();
        }
        part.start -= range.start;
        out.push(part);
    }
    (out, captured)
}

/// The material [`clear_range`] removes from `range`: [`copy_range`] without the time- and edit-locked
/// clips, which it keeps whole. Cut and the Move Selection commands carry this, so a move never leaves a
/// locked clip in place and also puts a copy of it elsewhere.
pub fn take_range(s: &Session, track: TrackId, range: Range) -> Vec<Clip> {
    copy_range_with_takes(s, None, None, track, range, true).0
}

/// Merge the MIDI inside `range` into one clip covering the whole range, gaps included, as it
/// sounds (muted clips stay silent). Returns false for tracks that aren't MIDI or instrument tracks.
pub fn consolidate_midi(timeline: &mut impl EditTimeline, track: TrackId, range: Range) -> bool {
    let mut s = timeline.clip_edit();
    let Some(tr) = s.track(track).filter(|t| t.kind.is_midi()) else { return false };
    let name = tr.name.clone();
    let source = tr.clips();
    let inputs = source.iter().filter(|c| !c.muted && c.range().intersect(&range).is_some());
    let mut partial_sources = Vec::new();
    let (score, take) = match (s.follow_grid, s.takes.as_deref()) {
        (Some(grid), Some(takes)) if inputs.clone().any(|c| takes.contains_key(&c.id)) => {
            // The remainder of an ordinary clip in this mixed edit must use the
            // same audible boundary as the portion going into the new clip.
            for clip in inputs.clone().filter(|c| !takes.contains_key(&c.id) && (c.start < range.start || c.end() > range.end)) {
                if let ClipContent::Midi { sequence } = &clip.content {
                    let rows = sequence.notes.iter().map(|&n| (n, Some(n))).collect();
                    let ctrls = sequence.ctrls.iter().map(|&c| (c, Some(c))).collect();
                    let (_, take) = crate::ClockTake::from_events(TimeCtx::of(&s), grid, clip.range(), rows, ctrls);
                    partial_sources.push((clip.id, take));
                }
            }
            let (score, take) = compose_clock_midi(&s, grid, range, inputs.map(|c| (c, takes.get(&c.id))));
            (score, Some(take))
        }
        _ => (merge_sequences(source, &s.tempo, s.sample_rate, range), None),
    };
    let id = s.new_clip_id();
    if let (Some(take), Some(takes)) = (take, &mut s.takes) {
        takes.extend(partial_sources);
        takes.insert(id, take);
    }
    place_clip(&mut s, track, Clip::midi(id, name, range.start, range.len(), score));
    true
}

/// Merge the parts on one timeline, clipping the final notes and controllers to their visible spans.
fn merge_sequences(clips: &[Clip], grid: &TempoMap, sr: SampleRate, range: Range) -> soundcraft_midi::Sequence {
    let mut merged = soundcraft_midi::Sequence::default();
    let base = grid.samples_to_ticks(range.start, sr);
    for clip in clips.iter().filter(|c| !c.muted) {
        let Some(visible) = clip.range().intersect(&range) else { continue };
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        let window = MidiWindow::new(grid, sr, visible, clip.start, base);
        merged.notes.extend(sequence.notes.iter().filter_map(|&n| window.note(n)));
        merged.ctrls.extend(sequence.ctrls.iter().filter_map(|&c| window.ctrl(c)));
    }
    merged.sort();
    merged
}

/// Compose absolute-positioned MIDI clips into one set of editable events. Keep
/// each source's projected time attached to its own event, including overlaps.
pub(crate) fn compose_clock_midi<'a>(
    doc: &Session,
    grid: &TempoMap,
    range: Range,
    clips: impl IntoIterator<Item = (&'a Clip, Option<&'a crate::ClockTake>)>,
) -> (soundcraft_midi::Sequence, crate::ClockTake) {
    let mut notes = Vec::new();
    let mut ctrls = Vec::new();
    let sr = doc.sample_rate;
    let score_base = doc.tempo.samples_to_ticks(range.start, sr);
    let play_base = grid.samples_to_ticks(range.start, sr);
    let edit_tick = |played: i64| {
        let sample = grid.tick_to_samples(play_base.saturating_add(played), sr);
        doc.tempo.samples_to_ticks(sample, sr).saturating_sub(score_base).max(0)
    };
    for (clip, take) in clips {
        let Some(visible) = clip.range().intersect(&range) else { continue };
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        let score_window = MidiWindow::rendered(&doc.tempo, sr, visible, clip.start, score_base);
        let play_window = MidiWindow::rendered(grid, sr, visible, clip.start, play_base);
        let play_origin = grid.samples_to_ticks(clip.start, sr);
        let heard_outside_selection = |tick| {
            let at = grid.tick_to_samples(play_origin.saturating_add(tick), sr);
            clip.range().contains(at) && !visible.contains(at)
        };
        let projected = take.map(|take| take.played_events(sequence, clip.start, grid));
        for (i, &note) in sequence.notes.iter().enumerate() {
            let played = projected.as_ref().map_or(Some(note), |p| p.0[i]);
            if played.is_some_and(|n| heard_outside_selection(n.start)) {
                continue;
            }
            let played = played.and_then(|n| play_window.note(n));
            let editable = score_window.note(note).or_else(|| {
                played.map(|n| {
                    let start = edit_tick(n.start);
                    soundcraft_midi::Note { start, length: edit_tick(n.end()).saturating_sub(start).max(1), ..n }
                })
            });
            if let Some(editable) = editable {
                notes.push((editable, played))
            }
        }
        for (i, &ctrl) in sequence.ctrls.iter().enumerate() {
            let played = projected.as_ref().map_or(Some(ctrl), |p| p.1[i]);
            if played.is_some_and(|c| heard_outside_selection(c.tick)) {
                continue;
            }
            let played = played.and_then(|c| play_window.ctrl(c));
            let editable = score_window.ctrl(ctrl).or_else(|| played.map(|c| soundcraft_midi::CtrlEvent { tick: edit_tick(c.tick), ..c }));
            if let Some(editable) = editable {
                ctrls.push((editable, played))
            }
        }
    }
    crate::ClockTake::from_events(TimeCtx::of(doc), grid, range, notes, ctrls)
}

struct MidiWindow<'a> {
    ticks: std::ops::Range<i64>,
    relative: i64,
    samples: Option<(TimeCtx<'a>, Range, i64)>,
}
impl<'a> MidiWindow<'a> {
    fn new(grid: &'a TempoMap, sr: SampleRate, visible: Range, start: Samples, base: i64) -> Self {
        let origin = grid.samples_to_ticks(start, sr);
        Self {
            ticks: grid.samples_to_ticks(visible.start, sr).saturating_sub(origin)..grid.samples_to_ticks(visible.end, sr).saturating_sub(origin),
            relative: origin.saturating_sub(base),
            samples: None,
        }
    }
    /// For clock composition, compare attacks in the same sample domain as playback
    /// and the source split. The selected end can fall less than one tick after an attack.
    fn rendered(grid: &'a TempoMap, sr: SampleRate, visible: Range, start: Samples, base: i64) -> Self {
        Self { samples: Some((TimeCtx { tempo: grid, sr }, visible, grid.samples_to_ticks(start, sr))), ..Self::new(grid, sr, visible, start, base) }
    }
    fn contains(&self, tick: i64) -> bool {
        match self.samples {
            Some((t, range, origin)) => range.contains(t.tempo.tick_to_samples(origin.saturating_add(tick), t.sr)),
            None => self.ticks.contains(&tick),
        }
    }
    fn note(&self, mut n: soundcraft_midi::Note) -> Option<soundcraft_midi::Note> {
        if !self.contains(n.start) {
            return None;
        }
        n.length = n.length.min(self.ticks.end.saturating_sub(n.start).max(1));
        n.start = n.start.saturating_add(self.relative);
        Some(n)
    }
    fn ctrl(&self, mut c: soundcraft_midi::CtrlEvent) -> Option<soundcraft_midi::CtrlEvent> {
        if !self.contains(c.tick) {
            return None;
        }
        c.tick = c.tick.saturating_add(self.relative);
        Some(c)
    }
}

/// Insert `clips` (positions relative to 0) at `at`. Overwrites what's underneath unless
/// `shuffle`, which first makes room for `length` (see [`make_room`], whose refusal it returns).
pub fn paste(
    timeline: &mut impl EditTimeline,
    track: TrackId,
    at: Samples,
    clips: &[Clip],
    length: Samples,
    shuffle: bool,
) -> Result<Vec<ClipId>, &'static str> {
    let mut s = timeline.clip_edit();
    let length = length.max(clips.iter().map(Clip::end).max().unwrap_or(0));
    if shuffle {
        make_room(&mut s, track, at, length)?;
    }
    let mut ids = Vec::new();
    let mut placed = Vec::new();
    for c in clips {
        let mut nc = c.clone();
        nc.id = s.new_clip_id();
        nc.start += at;
        ids.push(nc.id);
        placed.push((track, nc));
    }
    // Clear every destination first, so pasted clips that overlap (crossfades) keep each other.
    put_clips(&mut s, placed);
    Ok(ids)
}

/// Place one clip, overwriting whatever it covers except time- and edit-locked clips, which it
/// overlaps (see [`clear_range`]).
pub fn place_clip(timeline: &mut impl EditTimeline, track: TrackId, clip: Clip) {
    let mut s = timeline.clip_edit();
    clear_range(&mut s, track, clip.range(), false);
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        pl.clips.push(clip);
        pl.sort();
    }
}

/// Insert silence: shift material at/after `range.start` right by the range length (Insert Time;
/// Shuffle mode edits use [`make_room`]).
pub fn insert_silence(timeline: &mut impl EditTimeline, track: TrackId, range: Range) {
    let mut s = timeline.clip_edit();
    if range.is_empty() {
        return;
    }
    separate_at(&mut s, track, range.start);
    shift_from(&mut s, track, range.start, range.len());
}

/// Move every clip that starts at or after `from` by `by`, locked or not.
pub fn shift_from(s: &mut Session, track: TrackId, from: Samples, by: Samples) {
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        for c in pl.clips.iter_mut().filter(|c| c.start >= from) {
            c.start = c.start.saturating_add(by);
        }
    }
}

/// Time- or edit-locked: the clip doesn't move, and Shuffle stops at it.
pub fn locked(c: &Clip) -> bool {
    c.time_locked || c.edit_locked
}

/// A clip's place on its track, for working out Shuffle layouts without touching the clips.
#[derive(Clone, Copy)]
struct Span {
    id: ClipId,
    start: Samples,
    end: Samples,
    locked: bool,
}

fn spans(clips: &[Clip]) -> Vec<Span> {
    clips.iter().map(|c| Span { id: c.id, start: c.start, end: c.end(), locked: locked(c) }).collect()
}

/// Shuffle mode's ripple: the spans from `from` on move by `by`, up to the first locked one, which
/// stays put with everything after it. A move left stops at the session start, keeping their
/// spacing. Returns how far they moved and where the lock is (`Samples::MAX` when none).
fn ripple_spans(spans: &mut [Span], from: Samples, by: Samples) -> (Samples, Samples) {
    let lock = spans.iter().filter(|c| c.locked && c.start >= from).map(|c| c.start).min().unwrap_or(Samples::MAX);
    let moves = |c: &Span| c.start >= from && c.start < lock;
    let floor = spans.iter().filter(|c| moves(c)).map(|c| c.start).min().unwrap_or(0).max(0);
    let by = by.max(-floor);
    for c in spans.iter_mut().filter(|c| moves(c)) {
        c.start = c.start.saturating_add(by);
        c.end = c.end.saturating_add(by);
    }
    (by, lock)
}

/// [`ripple_spans`] on a track's clips.
fn ripple(s: &mut Session, track: TrackId, from: Samples, by: Samples) -> (Samples, Samples) {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return (0, Samples::MAX) };
    let mut sp = spans(&pl.clips);
    let moved = ripple_spans(&mut sp, from, by);
    for (c, n) in pl.clips.iter_mut().zip(&sp) {
        c.start = n.start;
    }
    pl.sort();
    moved
}

/// Automation Follows Edit for a Shuffle ripple (`moved` from [`ripple`]): the points from `from`
/// up to the lock move with the clips, and when material closes up the points in the span that
/// goes away go too.
fn follow_automation(s: &mut Session, track: TrackId, from: Samples, (by, lock): (Samples, Samples)) {
    if !s.edit.automation_follows_edit || by == 0 {
        return;
    }
    let Some(t) = s.track_mut(track) else { return };
    for l in &mut t.automation {
        if by < 0 {
            l.clear_range(from.saturating_add(by), from);
        }
        for p in l.points.iter_mut().filter(|p| p.at >= from && p.at < lock) {
            p.at = p.at.saturating_add(by);
        }
        l.points.sort_by_key(|p| p.at);
        l.points.dedup_by_key(|p| p.at);
    }
}

/// With Automation Follows Edit, take the points in `[start, end)` off a track, timed from `origin`.
fn take_automation(s: &mut Session, track: TrackId, start: Samples, end: Samples, origin: Samples) -> Vec<(AutoParam, AutomationPoint)> {
    let mut out = Vec::new();
    if !s.edit.automation_follows_edit || end <= start {
        return out;
    }
    let Some(t) = s.track_mut(track) else { return out };
    for l in &mut t.automation {
        let taken = l.points.iter().filter(|p| p.at >= start && p.at < end);
        out.extend(taken.map(|p| (l.param.clone(), AutomationPoint { at: p.at.saturating_sub(origin), value: p.value })));
        l.clear_range(start, end);
    }
    out
}

fn put_automation(s: &mut Session, track: TrackId, origin: Samples, points: &[(AutoParam, AutomationPoint)]) {
    let Some(t) = s.track_mut(track) else { return };
    for (param, p) in points {
        t.lane_mut(param).set_point(origin.saturating_add(p.at), p.value);
    }
}

/// How far Shuffle mode can push the material from `at` on before it runs into a locked clip
/// (`Samples::MAX` when none follows).
pub fn shuffle_room(s: &Session, track: TrackId, at: Samples) -> Samples {
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    let Some(lock) = clips.iter().filter(|c| locked(c) && c.end() > at).map(|c| c.start).min() else { return Samples::MAX };
    let end = clips.iter().filter(|c| c.end() > at && c.start < lock).map(Clip::end).fold(at, Samples::max);
    lock.saturating_sub(end)
}

/// Why Shuffle can't make room: a locked clip follows too closely.
pub const NO_ROOM: &str = "not enough room before a locked clip";
/// Why Shuffle can't make room: the point is inside a crossfade, which can't be pulled apart.
pub const IN_CROSSFADE: &str = "can't make room inside a crossfade";

/// Shuffle mode's room for `len` at `at`: later clips (and with Automation Follows Edit their
/// automation) move right. Refused, with nothing changed, when a locked clip is in the way or
/// `at` is inside a crossfade.
pub fn make_room(timeline: &mut impl EditTimeline, track: TrackId, at: Samples, len: Samples) -> Result<(), &'static str> {
    let mut s = timeline.clip_edit();
    if len <= 0 {
        return Ok(());
    }
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    if clips.iter().filter(|c| c.start <= at && at < c.end()).count() > 1 {
        return Err(IN_CROSSFADE);
    }
    if shuffle_room(&s, track, at) < len {
        return Err(NO_ROOM);
    }
    separate_at(&mut s, track, at);
    let moved = ripple(&mut s, track, at, len);
    follow_automation(&mut s, track, at, moved);
    Ok(())
}

/// Rebase both score and followed events when a trim keeps the clip ID.
fn trim_left(clip: &mut Clip, at: Samples, time: TimeCtx<'_>, grid: Option<&TempoMap>, takes: Option<&mut crate::ClockTakes>) {
    let old = clip.range();
    clip.trim_start_to(at);
    if clip.start == old.start {
        return;
    }
    let take = takes.and_then(|takes| takes.get_mut(&clip.id));
    let range = clip.range();
    if let (Some(take), Some(grid), ClipContent::Midi { sequence }) = (take, grid, &mut clip.content) {
        (*sequence, *take) = take.slice_events(sequence, old, range, time, grid);
    } else {
        rebase_midi_start(clip, old.start, time);
    }
}

/// Trim clips to keep only material inside `range`.
pub fn trim_to(timeline: &mut impl EditTimeline, track: TrackId, range: Range) -> usize {
    let ClipEdit { doc: s, mut takes, follow_grid } = timeline.clip_edit();
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        // Locked clips keep their extent, like they don't move for nudge.
        if locked(c) {
            continue;
        }
        if let Some(i) = c.range().intersect(&range)
            && i != c.range()
        {
            trim_left(c, i.start, tc, follow_grid, takes.as_deref_mut());
            c.trim_end_to(i.end);
            n += 1;
        }
    }
    pl.sort();
    n
}

/// Trim the start of clips under `at` to `at` (Trim Start to Insertion).
pub fn trim_start_to(timeline: &mut impl EditTimeline, track: TrackId, at: Samples) -> usize {
    let ClipEdit { doc: s, mut takes, follow_grid } = timeline.clip_edit();
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if locked(c) {
            continue;
        }
        if c.range().contains(at) && at > c.start {
            trim_left(c, at, tc, follow_grid, takes.as_deref_mut());
            n += 1;
        }
    }
    n
}

/// Trim the end of clips after `at` to `at` (Trim End to Insertion).
pub fn trim_end_to(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if locked(c) {
            continue;
        }
        if at > c.start && at < c.end() {
            c.trim_end_to(at);
            n += 1;
        }
    }
    n
}

/// Source samples covered by `length` timeline samples of a clip with the given `stretch`.
fn source_len(length: Samples, stretch: f64) -> Samples {
    if stretch.is_finite() && stretch > 0.0 && (stretch - 1.0).abs() >= 1e-9 { (length as f64 / stretch).round() as Samples } else { length }
}

/// Heal separations inside `range`: merge neighbouring audio clips that continue the same file,
/// either touching (a split) or with an empty gap that matches the gap in the source (a cut).
pub fn heal(s: &mut Session, track: TrackId, range: Range) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let mut out: Vec<Clip> = Vec::with_capacity(pl.clips.len());
    let mut healed = 0;
    for c in pl.clips.drain(..) {
        if let Some(prev) = out.last_mut() {
            let contiguous = c.start >= prev.end() && prev.end() >= range.start && c.start <= range.end;
            let gap = if contiguous { c.start.saturating_sub(prev.end()) } else { 0 };
            let same = match (&prev.content, &c.content) {
                (ClipContent::Audio { source: a, offset: oa }, ClipContent::Audio { source: b, offset: ob }) => {
                    a == b && oa.saturating_add(source_len(prev.length.saturating_add(gap), prev.stretch)) == *ob
                }
                _ => false,
            };
            let same_playback = prev.muted == c.muted && (prev.stretch - c.stretch).abs() < 1e-9 && (prev.gain_db - c.gain_db).abs() < 1e-6;
            if contiguous && same && same_playback {
                if !prev.gain_env.is_empty() || !c.gain_env.is_empty() {
                    join_gain_env(prev, &c);
                }
                prev.length = c.end().saturating_sub(prev.start);
                prev.fade_out = c.fade_out;
                healed += 1;
                continue;
            }
        }
        out.push(c);
    }
    pl.clips = out;
    healed
}

/// Give `l` the gain envelope of `l` followed by `r`, which starts at or after where `l` ends, so
/// every sample keeps its gain. Call it before `l.length` grows to cover `r`.
fn join_gain_env(l: &mut Clip, r: &Clip) {
    let cut = l.length;
    let seam = r.start.saturating_sub(l.start);
    let last = cut.saturating_sub(1);
    let (end_db, start_db) = (l.env_db_at(last), r.env_db_at(0));
    l.gain_env.retain(|&(at, _)| at < cut);
    l.gain_env.extend(r.gain_env.iter().filter(|&&(at, _)| at >= 0).map(|&(at, db)| (at.saturating_add(seam), db)));
    // Pin both sides of the seam where the joined points would interpolate across it.
    for (at, db) in [(seam, start_db), (last, end_db)] {
        if (l.env_db_at(at) - db).abs() > 1e-4 {
            let i = l.gain_env.partition_point(|&(t, _)| t < at);
            l.gain_env.insert(i, (at, db));
        }
    }
}

/// Move clips by `delta`, overwriting what they land on; `dest` sends whole tracks of them
/// elsewhere (see [`destinations`]). Same-track moves clamped to zero leave all material intact.
pub fn move_clips(timeline: &mut impl EditTimeline, ids: &[ClipId], delta: Samples, dest: &[(TrackId, TrackId)]) -> usize {
    let mut s = timeline.clip_edit();
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    // Limit the shift before removing any clips, so a stationary edit cannot overwrite overlaps.
    let limit = s.tracks.iter().flat_map(|t| t.clips()).filter(|c| ids.contains(&c.id) && !locked(c)).map(|c| c.start.saturating_neg()).max();
    let delta = delta.max(limit.unwrap_or(0));
    if delta == 0 && dest.iter().all(|(from, to)| from == to) {
        return 0;
    }
    let mut moving: Vec<(TrackId, Clip)> = Vec::new();
    for t in &mut s.tracks {
        let tid = t.id;
        if let Some(pl) = t.playlist_mut() {
            pl.clips.retain(|c| {
                if ids.contains(&c.id) && !locked(c) {
                    moving.push((tid, c.clone()));
                    false
                } else {
                    true
                }
            });
        }
    }
    let n = moving.len();
    let placed = moving.into_iter().map(|(tid, mut c)| {
        c.start = c.start.saturating_add(delta);
        (lands(dest, tid), c)
    });
    put_clips(&mut s, placed.collect());
    n
}

/// Place moved clips, overwriting what they cover but not each other, so a crossfaded pair
/// moved together stays a crossfade.
fn put_clips(timeline: &mut impl EditTimeline, placed: Vec<(TrackId, Clip)>) {
    let mut s = timeline.clip_edit();
    for (t, c) in &placed {
        clear_range(&mut s, *t, c.range(), false);
    }
    for (t, c) in placed {
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.push(c);
            pl.sort();
        }
    }
}

/// Whether the Edit window shows track `t`: not hidden, and not inside a closed folder at any level.
pub fn shown(s: &Session, t: &Track) -> bool {
    s.track_shown(t)
}

/// Where the tracks of clips `ids` go when track `from` goes to `to`: each keeps its distance
/// from it among the tracks the Edit window shows, or among all tracks when a hidden one is
/// involved. None when one would land past either end, or on a track that can't hold its clips.
pub fn destinations(s: &Session, ids: &[ClipId], from: TrackId, to: TrackId) -> Option<Vec<(TrackId, TrackId)>> {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let holds = |t: &Track| t.clips().iter().any(|c| ids.contains(&c.id));
    let visible = s.tracks.iter().filter(|t| t.id == from || t.id == to || holds(t)).all(|t| shown(s, t));
    let rows: Vec<&Track> = s.tracks.iter().filter(|t| !visible || shown(s, t)).collect();
    let row = |id: TrackId| rows.iter().position(|t| t.id == id).and_then(|i| isize::try_from(i).ok());
    let shift = row(to)? - row(from)?;
    s.tracks
        .iter()
        .filter(|t| holds(t))
        .map(|t| {
            let d = rows.get(usize::try_from(row(t.id)? + shift).ok()?)?;
            (d.kind.has_playlist() && (d.kind == t.kind || d.kind.is_midi() && t.kind.is_midi())).then_some((t.id, d.id))
        })
        .collect()
}

/// The track that clips on `t` land on under a [`destinations`] map.
pub fn lands(dest: &[(TrackId, TrackId)], t: TrackId) -> TrackId {
    dest.iter().find(|(from, _)| *from == t).map_or(t, |(_, d)| *d)
}

/// The earliest of clips `ids` that can move (neither time- nor edit-locked).
pub fn first_movable<'a>(s: &'a Session, ids: &[ClipId]) -> Option<&'a Clip> {
    ids.iter().filter_map(|id| s.find_clip(*id)).map(|(_, c)| c).filter(|c| !locked(c)).min_by_key(|c| c.start)
}

/// One Shuffle lift: where the hole the clip leaves starts and how long it is, how far the clips
/// after it actually close up (none past a lock, none before the session start) and where the
/// first locked clip after it is.
#[derive(Clone, Copy)]
struct Lift {
    id: ClipId,
    start: Samples,
    from: Samples,
    shift: Samples,
    by: Samples,
    lock: Samples,
}

/// Lifting the movable `moving` clips off a track's `spans` the Shuffle way, right to left, in
/// one pass: the lifts in that order, and the spans that stay at the starts they close up to.
/// A hole starts where the clips before it end (a crossfade partner keeps its material) and
/// closes up to the next clip, so silence after the lifted clip stays. Clips are ordered by
/// start, then id, so of two stacked clips the later one counts as after the other.
fn lift_plan(mut sp: Vec<Span>, moving: &HashSet<ClipId>) -> (Vec<Lift>, Vec<Span>) {
    sp.sort_by_key(|c| (c.start, c.id));
    let lifted = |c: &Span| !c.locked && moving.contains(&c.id);
    let reach: Vec<Samples> = std::iter::once(Samples::MIN)
        .chain(sp.iter().scan(Samples::MIN, |far, c| {
            *far = (*far).max(c.end);
            Some(*far)
        }))
        .collect();
    let mut lifts = Vec::new();
    let (mut next, mut next_moves, mut lock) = (None, false, Samples::MAX);
    for (i, c) in sp.iter().enumerate().rev() {
        if !lifted(c) {
            (next, next_moves) = (Some(c.start), !c.locked);
            if c.locked {
                lock = c.start;
            }
            continue;
        }
        let from = reach.get(i).map_or(c.start, |far| c.start.max(*far));
        let shift = next.unwrap_or(c.end).min(c.end).saturating_sub(from).max(0);
        let by = if next_moves { -shift.min(next.unwrap_or(0).max(0)) } else { 0 };
        next = next.map(|n| n.saturating_add(by));
        lifts.push(Lift { id: c.id, start: c.start, from, shift, by, lock });
    }
    let (mut by, mut pending) = (0, lifts.iter().rev());
    let mut staying = Vec::with_capacity(sp.len() - lifts.len());
    for c in sp {
        if lifted(&c) {
            by += pending.next().map_or(0, |l| l.by);
        } else if c.locked {
            by = 0;
            staying.push(c);
        } else {
            staying.push(Span { start: c.start.saturating_add(by), end: c.end.saturating_add(by), ..c });
        }
    }
    (lifts, staying)
}

/// Where Shuffle can land material on `track` once the movable `moving` clips have left it (later
/// clips close each gap): the clip boundaries, never inside a clip or a crossfade, sorted. Empty
/// for an empty track.
pub fn shuffle_slots(s: &Session, track: TrackId, moving: &[ClipId]) -> Vec<Samples> {
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    let (_, mut sp) = lift_plan(spans(clips), &moving.iter().copied().collect());
    sp.sort_by_key(|c| c.start);
    let reach: Vec<Samples> = sp
        .iter()
        .scan(Samples::MIN, |far, c| {
            *far = (*far).max(c.end);
            Some(*far)
        })
        .collect();
    let inside = |b: Samples| reach.get(sp.partition_point(|c| c.start < b).wrapping_sub(1)).is_some_and(|far| *far > b);
    let mut slots: Vec<Samples> = sp.iter().flat_map(|c| [c.start, c.end]).filter(|b| !inside(*b)).collect();
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// The slot nearest `at` (the earlier one on a tie), or the session start when there are none.
pub fn nearest_slot(slots: &[Samples], at: Samples) -> Samples {
    let i = slots.partition_point(|b| *b < at);
    [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|i| slots.get(i)).copied().min_by_key(|b| b.abs_diff(at)).unwrap_or(0)
}

/// Where Shuffle mode lands material dropped at `at` on `track` (see [`shuffle_slots`]).
pub fn shuffle_slot(s: &Session, track: TrackId, at: Samples, moving: &[ClipId]) -> Samples {
    nearest_slot(&shuffle_slots(s, track, moving), at)
}

/// Lift the movable `moving` clips off a track the Shuffle way (see [`lift_plan`]); with
/// Automation Follows Edit their automation leaves with them. Returns the clips, each with that
/// automation timed from its start.
fn lift_all(s: &mut Session, track: TrackId, moving: &HashSet<ClipId>) -> Vec<(Clip, Vec<(AutoParam, AutomationPoint)>)> {
    let Some(clips) = s.track(track).map(|t| spans(t.clips())) else { return Vec::new() };
    let (lifts, staying) = lift_plan(clips, moving);
    let mut points: HashMap<ClipId, Vec<(AutoParam, AutomationPoint)>> = HashMap::new();
    for l in &lifts {
        points.insert(l.id, take_automation(s, track, l.from, l.from.saturating_add(l.shift), l.start));
        follow_automation(s, track, l.from.saturating_add(l.shift), (l.by, l.lock));
    }
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return Vec::new() };
    let starts: HashMap<ClipId, Samples> = staying.iter().map(|c| (c.id, c.start)).collect();
    let (out, kept): (Vec<Clip>, Vec<Clip>) = std::mem::take(&mut pl.clips).into_iter().partition(|c| points.contains_key(&c.id));
    pl.clips = kept;
    for c in &mut pl.clips {
        if let Some(start) = starts.get(&c.id) {
            c.start = *start;
        }
    }
    pl.sort();
    out.into_iter()
        .map(|c| {
            let p = points.remove(&c.id).unwrap_or_default();
            (c, p)
        })
        .collect()
}

/// The movable clips `ids` grouped by the track they land on (per `dest`): each destination and
/// the start of its earliest clip, which Shuffle lands on a slot; the others keep their distance.
pub fn shuffle_groups(s: &Session, ids: &[ClipId], dest: &[(TrackId, TrackId)]) -> Vec<(TrackId, Samples)> {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let mut groups: Vec<(TrackId, Samples)> = Vec::new();
    for t in &s.tracks {
        for c in t.clips().iter().filter(|c| ids.contains(&c.id) && !locked(c)) {
            let d = lands(dest, t.id);
            match groups.iter_mut().find(|g| g.0 == d) {
                Some(g) => g.1 = g.1.min(c.start),
                None => groups.push((d, c.start)),
            }
        }
    }
    groups
}

/// Move clips in Shuffle mode: each leaves its track (later clips close the gap, see
/// [`lift_plan`]), and the clips bound for each track land together, the earliest on the
/// [`shuffle_slot`] nearest `delta` away, pushing later clips right. With Automation Follows Edit
/// their automation travels with them. Refused, with the reason, when a locked clip or a
/// crossfade leaves no room.
pub fn shuffle_clips(timeline: &mut impl EditTimeline, ids: &[ClipId], delta: Samples, dest: &[(TrackId, TrackId)]) -> Result<usize, &'static str> {
    let mut s = timeline.clip_edit();
    let groups: Vec<(TrackId, Samples, Samples)> =
        shuffle_groups(&s, ids, dest).into_iter().map(|(d, first)| (d, first, shuffle_slot(&s, d, first.saturating_add(delta), ids))).collect();
    let moving: HashSet<ClipId> = ids.iter().copied().collect();
    let sources: Vec<TrackId> = s.tracks.iter().filter(|t| t.clips().iter().any(|c| moving.contains(&c.id) && !locked(c))).map(|t| t.id).collect();
    let mut lifted = Vec::new();
    for t in sources {
        lifted.extend(lift_all(&mut s, t, &moving).into_iter().map(|(c, points)| (lands(dest, t), c, points)));
    }
    for (d, first, at) in groups {
        let block: Vec<&(TrackId, Clip, Vec<(AutoParam, AutomationPoint)>)> = lifted.iter().filter(|l| l.0 == d).collect();
        let len = block.iter().map(|l| l.1.end()).max().unwrap_or(first).saturating_sub(first);
        make_room(&mut s, d, at, len)?;
        let mut placed = Vec::new();
        for (_, c, points) in block {
            let mut c = c.clone();
            c.start = at.saturating_add(c.start.saturating_sub(first));
            put_automation(&mut s, d, c.start, points);
            placed.push(c);
        }
        if let Some(pl) = s.track_mut(d).and_then(|t| t.playlist_mut()) {
            pl.clips.extend(placed);
            pl.sort();
        }
    }
    Ok(lifted.len())
}

/// How far a Grabber drag by `raw` moves `clip` in Grid mode: Absolute puts its sync point (its
/// start when it has none inside the clip) on the nearest grid line; Relative moves it by whole
/// grid steps, keeping its offset from the grid.
pub fn grid_delta(s: &Session, clip: &Clip, raw: Samples, relative: bool, moving: &[ClipId]) -> Samples {
    let sync = if (0..clip.length).contains(&clip.sync_point) { clip.sync_point } else { 0 };
    let anchor = clip.start.saturating_add(sync);
    let from = if relative { s.grid_line_at_or_before_excluding(anchor, moving) } else { anchor };
    s.snap_to_grid_excluding(from.saturating_add(raw), moving).saturating_sub(from)
}

/// Trim a clip's edges to `start` and/or `end` like the Trim tool, never past its file. In
/// Shuffle mode the start stays put (the material slides under it) and later clips follow the
/// change, up to a locked clip; otherwise a trim stops at the neighbouring clips.
pub fn trim_clip(timeline: &mut impl EditTimeline, id: ClipId, start: Option<Samples>, end: Option<Samples>, shuffle: bool) -> Option<Clip> {
    let ClipEdit { doc: s, takes, follow_grid } = timeline.clip_edit();
    let (track, old) = s.find_clip(id).filter(|(_, c)| !c.edit_locked).map(|(t, c)| (t, c.clone()))?;
    let (mut lo, mut hi) = (Samples::MIN, Samples::MAX);
    if shuffle {
        let room = shuffle_room(s, track, old.end()).max(0);
        lo = old.start.saturating_sub(room);
        hi = old.end().saturating_add(room);
    } else {
        let others = s.track(track).map(|t| t.clips()).unwrap_or_default().iter().filter(|c| c.id != id);
        if let Some(prev) = others.clone().filter(|c| c.start < old.start).map(Clip::end).max() {
            lo = lo.max(prev.min(old.start));
        }
        if let Some(next) = others.filter(|c| c.start > old.start).map(|c| c.start).min() {
            hi = hi.min(next.max(old.end()));
        }
        lo = lo.max(0);
    }
    let file = |c: &Clip| {
        // The mixer plays from `offset + rel / stretch`, so file samples span `stretch` times as many timeline samples.
        let stretch = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 };
        let scale = |n: Samples| if (stretch - 1.0).abs() >= 1e-9 { (n as f64 * stretch).round() as Samples } else { n };
        let start = c.start.saturating_sub(scale(c.source_offset()));
        material_len(s, c).map(|n| (start.min(c.start), start.saturating_add(scale(n)).max(c.end())))
    };
    let t = TimeCtx::of(s);
    let mut c = old.clone();
    if let Some(at) = start {
        let at = at.max(file(&c).map_or(lo, |(f, _)| lo.max(f)));
        trim_left(&mut c, at, t, follow_grid, takes);
        if shuffle {
            c.start = old.start;
        }
    }
    if let Some(at) = end {
        c.trim_end_to(at.min(file(&c).map_or(hi, |(_, f)| hi.min(f))));
    }
    let pl = s.track_mut(track).and_then(|t| t.playlist_mut())?;
    *pl.clip_mut(id)? = c.clone();
    pl.sort();
    if shuffle {
        let moved = ripple(s, track, old.start.saturating_add(1), c.end().saturating_sub(old.end()));
        follow_automation(s, track, old.end(), moved);
    }
    Some(c)
}

/// Length of the file or movie under a clip; None for MIDI, which has no end.
fn material_len(s: &Session, c: &Clip) -> Option<Samples> {
    match c.content {
        ClipContent::Audio { source, .. } => s.source(source).map(|src| i64::try_from(src.frames).unwrap_or(i64::MAX)),
        ClipContent::Video { source, .. } => s.video(source).map(|v| s.sample_rate.samples(v.duration.clamp(0.0, 1e7))),
        ClipContent::Midi { .. } => None,
    }
}

/// Create fades from a selection (the incumbent's Edit › Fades › Create semantics):
/// a range across a clip boundary makes a crossfade; at a clip start a fade-in; at its end a fade-out.
pub fn create_fades(s: &mut Session, track: TrackId, range: Range, shape: FadeShape) -> usize {
    if range.is_empty() {
        return 0;
    }
    let handles: Vec<(ClipId, Samples)> = {
        let Some(tr) = s.track(track) else { return 0 };
        tr.clips().iter().map(|c| (c.id, material_len(s, c).map_or(0, |n| n.saturating_sub(c.source_offset()).saturating_sub(c.length)))).collect()
    };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let mut made = 0;
    let mut xfaded: Vec<ClipId> = Vec::new();
    let n = pl.clips.len();
    // Crossfades between adjacent clips whose boundary is inside the range.
    for i in 0..n.saturating_sub(1) {
        let (Some(a), Some(b)) = (pl.clips.get(i).cloned(), pl.clips.get(i + 1).cloned()) else { continue };
        let boundary = a.end();
        if b.start == boundary && range.start < boundary && range.end > boundary {
            let before = boundary - range.start.max(a.start);
            let after = range.end.min(b.end()) - boundary;
            let a_handle = handles.iter().find(|h| h.0 == a.id).map_or(0, |h| h.1);
            let b_handle = b.source_offset();
            // Extend each clip into the other's territory using its handles.
            let ext_a = after.min(a_handle).max(0);
            let ext_b = before.min(b_handle).max(0);
            if let Some(ca) = pl.clips.get_mut(i) {
                ca.length += ext_a;
                ca.fade_out = Fade { len: (ext_a + before).min(ca.length), shape, curvature: 0.0 };
            }
            if let Some(cb) = pl.clips.get_mut(i + 1) {
                let old = cb.start;
                cb.trim_start_to(old - ext_b);
                cb.fade_in = Fade { len: (ext_b + after).min(cb.length), shape, curvature: 0.0 };
            }
            made += 1;
            xfaded.push(a.id);
            xfaded.push(b.id);
        }
    }
    for c in pl.clips.iter_mut().filter(|c| !xfaded.contains(&c.id)) {
        if range.start <= c.start && range.end > c.start && range.end < c.end() {
            c.fade_in = Fade { len: range.end - c.start, shape, curvature: 0.0 };
            made += 1;
        } else if range.end >= c.end() && range.start > c.start && range.start < c.end() {
            c.fade_out = Fade { len: c.end() - range.start, shape, curvature: 0.0 };
            made += 1;
        }
    }
    made
}

/// Remove fades on clips touching `range`.
pub fn delete_fades(s: &mut Session, track: TrackId, range: Range) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    let mut touched: HashSet<ClipId> = HashSet::new();
    for c in &mut pl.clips {
        if c.range().overlaps(&range) || (range.is_empty() && c.range().contains(range.start)) {
            touched.insert(c.id);
            if c.fade_in.len > 0 || c.fade_out.len > 0 {
                n += 1;
            }
            c.fade_in = Fade::default();
            c.fade_out = Fade::default();
        }
    }
    // Overlapping (crossfaded) neighbours in the range are trimmed back to the midpoint; other overlaps stay as they are.
    pl.sort();
    for i in 0..pl.clips.len().saturating_sub(1) {
        let (Some(a_end), Some(b_start)) = (pl.clips.get(i).map(Clip::end), pl.clips.get(i + 1).map(|c| c.start)) else { continue };
        let in_range = pl.clips.get(i).is_some_and(|a| touched.contains(&a.id)) || pl.clips.get(i + 1).is_some_and(|b| touched.contains(&b.id));
        if in_range && a_end > b_start {
            let mid = b_start + (a_end - b_start) / 2;
            if let Some(a) = pl.clips.get_mut(i) {
                a.trim_end_to(mid);
            }
            if let Some(b) = pl.clips.get_mut(i + 1) {
                b.trim_start_to(mid);
            }
        }
    }
    // Trimming can move a start past a later clip's start when one clip overlapped several.
    pl.sort();
    n
}

#[cfg(test)]
mod tests;
